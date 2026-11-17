//! Transform interpolation for entities simulated at the fixed rate.
//!
//! An entity moved in `FixedUpdate` (60 Hz) changes position only on frames
//! where a step runs, so on a 120/144/240 Hz display it visibly stutters. An
//! [`Interpolated`] entity is drawn between its last two fixed-step
//! transforms instead, blended by [`FixedTime::alpha`]:
//!
//! 1. at the end of every fixed step the app records the entity's
//!    `Transform` (the previous record becomes "previous");
//! 2. at the end of `PostUpdate`, after every system (renderers read the
//!    world after that), the app writes the blended world transform into
//!    `GlobalTransform` for the entity and, rigidly, for its descendants;
//! 3. at the start of the next `PostUpdate` the simulated `GlobalTransform`
//!    is restored, so transform propagation, physics sync and every other
//!    `PostUpdate` system work on simulated values.
//!
//! `Transform` is never touched: it always holds the simulation. Between
//! frames (in `FixedUpdate` and `Update`) `GlobalTransform` of interpolated
//! entities holds what was drawn; read `Transform`, or
//! [`interpolated_transform`] for what will be drawn this frame (a camera
//! following an interpolated player).
//!
//! **Snapping.** A move longer than [`Interpolated::teleport_distance`]
//! between two steps, a `Transform` changed outside `FixedUpdate` (an
//! `Update` system, the editor, a respawn written in `Update`, test code
//! between frames) and [`Interpolated::reset`] draw the new transform at
//! once instead of sliding there. The cost of drawing one step behind the simulation (at
//! most 1/60 s) is the price of smooth motion, as in every engine that
//! interpolates.
//!
//! [`FixedTime::alpha`]: crate::app::FixedTime::alpha

use glam::Affine3A;

use crate::reflect::{Range, Reflect, ReflectComponent, ReflectDefault, Unit};
use crate::transform::{Children, GlobalTransform, Propagated, Transform, same_local};
use crate::world::{Entity, World};

/// Draw this entity between its last two fixed-step transforms. For
/// entities moved in `FixedUpdate` (characters, projectiles, moving
/// platforms); see the [module docs](self). Saved in scenes.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Component, Default, Clone, Debug, PartialEq)]
#[type_path = "box_engine"]
pub struct Interpolated {
    /// A move longer than this between two fixed steps is a teleport: drawn
    /// at once instead of blended.
    #[reflect(@Range { min: 0.0, max: 1000.0 }, @Unit("m"))]
    pub teleport_distance: f32,
}

impl Default for Interpolated {
    fn default() -> Self {
        Self {
            teleport_distance: 10.0,
        }
    }
}

impl Interpolated {
    /// Draw `entity` at its current `Transform` from now on, without
    /// sliding from where it was (after a respawn or a teleport made in
    /// `FixedUpdate`). Nothing if it is not interpolated.
    pub fn reset(world: &mut World, entity: Entity) {
        let Some(t) = world.get::<Transform>(entity).map(|t| *t) else {
            return;
        };
        if let Some(s) = world.component_mut::<InterpolationState>(entity) {
            s.previous = t;
            s.current = t;
        }
    }
}

/// Runtime record of an [`Interpolated`] entity (never saved).
#[derive(Clone, Copy, Debug)]
struct InterpolationState {
    previous: Transform,
    current: Transform,
    /// `GlobalTransform` of the entity and its descendants holds a blend.
    blended: bool,
}

fn lerp(a: &Transform, b: &Transform, t: f32) -> Transform {
    Transform {
        translation: a.translation.lerp(b.translation, t),
        rotation: a.rotation.slerp(b.rotation, t),
        scale: a.scale.lerp(b.scale, t),
    }
}

/// The local transform drawn this frame for an [`Interpolated`] entity
/// (its `Transform` for any other entity, or one that snaps this frame).
/// Valid once this frame's fixed steps ran, so `Update` systems can follow
/// an interpolated entity smoothly. `None` without a `Transform`.
pub fn interpolated_transform(world: &World, entity: Entity) -> Option<Transform> {
    let t = *world.get::<Transform>(entity)?;
    let Some(s) = world.get::<InterpolationState>(entity).map(|s| *s) else {
        return Some(t);
    };
    if !world.has::<Interpolated>(entity) || !same_local(Some(&t), Some(&s.current)) {
        return Some(t);
    }
    let alpha = world
        .get_resource::<crate::app::FixedTime>()
        .map_or(1.0, |f| f.alpha());
    Some(lerp(&s.previous, &s.current, alpha))
}

/// Start of a fixed step: a `Transform` that differs from the last record
/// was changed outside `FixedUpdate` (a teleport from `Update`, the editor,
/// a test): start blending from there instead of sliding to it.
pub(crate) fn before_step(world: &mut World) {
    for (t, s) in world
        .query_mut::<(&Transform, &mut InterpolationState)>()
        .with::<&Interpolated>()
    {
        if !same_local(Some(t), Some(&s.current)) {
            s.previous = *t;
            s.current = *t;
        }
    }
}

/// End of a fixed step: record the simulated transforms.
pub(crate) fn record(world: &mut World) {
    let mut new: Vec<(Entity, InterpolationState)> = Vec::new();
    for (e, interp, t, state) in world
        .query::<(
            Entity,
            &Interpolated,
            &Transform,
            Option<&mut InterpolationState>,
        )>()
        .iter()
    {
        match state {
            Some(s) => {
                let jump = t.translation.distance(s.current.translation);
                let teleport = jump.is_nan() || jump > interp.teleport_distance;
                s.previous = if teleport { *t } else { s.current };
                s.current = *t;
            }
            None => new.push((
                e,
                InterpolationState {
                    previous: *t,
                    current: *t,
                    blended: false,
                },
            )),
        }
    }
    for (e, s) in new {
        let _ = world.ecs_mut().insert_one(e, s);
    }
}

/// The simulated world transform of `e` as last propagated.
fn simulated(world: &World, e: Entity) -> Affine3A {
    world
        .get::<Propagated>(e)
        .map(|p| p.global)
        .unwrap_or_else(|| crate::transform::compute_global(world, e))
}

/// Write `f(simulated global)` into the `GlobalTransform` of every
/// descendant of `root`.
fn for_descendants(world: &World, root: Entity, f: impl Fn(Affine3A) -> Affine3A) {
    let mut stack: Vec<Entity> = world
        .get::<Children>(root)
        .map(|c| c.0.clone())
        .unwrap_or_default();
    // Bounded so a cycle made through raw hecs access cannot hang.
    let mut budget = world.entity_count() as usize;
    while let Some(e) = stack.pop() {
        if budget == 0 {
            break;
        }
        budget -= 1;
        if let (Some(p), Some(mut g)) = (
            world.get::<Propagated>(e),
            world.get_mut::<GlobalTransform>(e),
        ) {
            g.0 = f(p.global);
        }
        if let Some(children) = world.get::<Children>(e) {
            stack.extend(children.0.iter().copied());
        }
    }
}

/// Start of `PostUpdate`: put the simulated `GlobalTransform` back where a
/// blend was drawn, and forget entities that lost [`Interpolated`].
pub(crate) fn restore(world: &mut World) {
    let mut blended: Vec<Entity> = Vec::new();
    let mut orphans: Vec<Entity> = Vec::new();
    for (e, s, interp) in world
        .query::<(Entity, &mut InterpolationState, Option<&Interpolated>)>()
        .iter()
    {
        if std::mem::take(&mut s.blended) {
            blended.push(e);
        }
        if interp.is_none() {
            orphans.push(e);
        }
    }
    for &e in &blended {
        let sim = simulated(world, e);
        if let Some(mut g) = world.get_mut::<GlobalTransform>(e) {
            g.0 = sim;
        }
        for_descendants(world, e, |sim| sim);
    }
    for e in orphans {
        let _ = world.ecs_mut().remove_one::<InterpolationState>(e);
    }
}

/// End of `PostUpdate`: draw interpolated entities `alpha` of the way from
/// their previous to their current fixed-step transform.
pub(crate) fn blend(world: &mut World, alpha: f32) {
    let mut draws: Vec<(Entity, Transform)> = Vec::new();
    for (e, t, s) in world
        .query::<(Entity, &Transform, &mut InterpolationState)>()
        .with::<&Interpolated>()
        .iter()
    {
        if !same_local(Some(t), Some(&s.current)) {
            // Moved outside FixedUpdate: snap.
            s.previous = *t;
            s.current = *t;
            continue;
        }
        if same_local(Some(&s.previous), Some(&s.current)) {
            continue;
        }
        s.blended = true;
        draws.push((e, lerp(&s.previous, &s.current, alpha)));
    }
    for (e, local) in draws {
        let parent = world
            .parent(e)
            .map_or(Affine3A::IDENTITY, |p| simulated(world, p));
        let drawn = parent * local.to_affine();
        let sim = simulated(world, e);
        let Some(mut g) = world.get_mut::<GlobalTransform>(e) else {
            continue;
        };
        g.0 = drawn;
        drop(g);
        let delta = drawn * sim.inverse();
        if delta.is_finite() {
            for_descendants(world, e, |sim| delta * sim);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, FixedTime, Stage};
    use glam::Vec3;

    /// An app moving every `Mover` +1 m along X per fixed step.
    fn app() -> (App, Entity, Entity) {
        let mut app = App::new();
        let e = app
            .world
            .spawn((Transform::default(), Interpolated::default(), MoverTag));
        let child = app.world.spawn((Transform::from_xyz(0.0, 1.0, 0.0),));
        app.world.set_parent(child, Some(e)).unwrap();
        app.add_system(Stage::FixedUpdate, |w| {
            for t in w.query_mut::<&mut Transform>().with::<&MoverTag>() {
                t.translation.x += 1.0;
            }
        });
        (app, e, child)
    }

    struct MoverTag;

    fn drawn(app: &App, e: Entity) -> Vec3 {
        app.world.get::<GlobalTransform>(e).unwrap().translation()
    }

    #[test]
    fn blends_by_alpha_at_144_hz_without_touching_the_simulation() {
        let (mut app, e, child) = app();
        let dt = 1.0 / 144.0;
        let mut seen = Vec::new();
        for _ in 0..12 {
            app.tick(dt);
            let fixed = *app.world.resource::<FixedTime>();
            let sim = app.world.get::<Transform>(e).unwrap().translation.x;
            seen.push((fixed.tick, fixed.alpha(), sim, drawn(&app, e).x));
        }
        for (i, &(tick, alpha, sim, x)) in seen.iter().enumerate() {
            assert_eq!(sim, tick as f32, "frame {i}: simulation untouched");
            // One step behind: previous + alpha of the last step, once two
            // steps have been recorded.
            let expected = if tick >= 2 {
                (tick - 1) as f32 + alpha
            } else {
                tick as f32
            };
            assert!(
                (x - expected).abs() < 1e-4,
                "frame {i}: drew {x}, expected {expected} (tick {tick}, alpha {alpha})"
            );
        }
        // Frame 5 is the first frame after the second step: 1 + 1/12.
        assert_eq!(seen[4].0, 2);
        assert!(
            (seen[4].3 - (1.0 + 1.0 / 12.0)).abs() < 1e-4,
            "{:?}",
            seen[4]
        );
        // The child follows rigidly.
        let d = drawn(&app, e);
        assert!((drawn(&app, child) - (d + Vec3::Y)).length() < 1e-5);
        // And between frames the drawn value is what Update sees, while
        // interpolated_transform gives the same blend.
        let t = interpolated_transform(&app.world, e).unwrap();
        assert!((t.translation - d).length() < 1e-5);
    }

    #[test]
    fn post_update_systems_and_propagation_see_the_simulation() {
        let (mut app, e, child) = app();
        app.world.insert_resource(Vec::<(f32, f32)>::new());
        app.add_system(Stage::PostUpdate, move |w| {
            let g = w.get::<GlobalTransform>(e).unwrap().translation().x;
            let c = w.get::<GlobalTransform>(child).unwrap().translation().x;
            w.resource_mut::<Vec<(f32, f32)>>().push((g, c));
        });
        for _ in 0..20 {
            app.tick(1.0 / 144.0);
        }
        let ticks = app.world.resource::<FixedTime>().tick as f32;
        let seen = app.world.resource::<Vec<(f32, f32)>>();
        assert_eq!(*seen.last().unwrap(), (ticks, ticks));
        assert!(
            seen.iter().all(|(g, c)| g == c && g.fract() == 0.0),
            "{seen:?}"
        );
    }

    #[test]
    fn teleports_and_moves_outside_fixed_update_snap() {
        let (mut app, e, _) = app();
        app.run_frames(10, 1.0 / 144.0);
        // A long jump inside FixedUpdate.
        app.add_system(Stage::FixedUpdate, move |w| {
            if w.resource::<FixedTime>().tick == 6 {
                w.component_mut::<Transform>(e).unwrap().translation.x = 100.0;
            }
        });
        for _ in 0..12 {
            app.tick(1.0 / 144.0);
            let tick = app.world.resource::<FixedTime>().tick;
            let x = drawn(&app, e).x;
            if tick == 6 {
                assert_eq!(x, 100.0, "snapped, not blended from 5");
            }
        }
        // Written in Update: drawn there at once.
        app.add_system(Stage::Update, move |w| {
            w.component_mut::<Transform>(e).unwrap().translation.y = 7.0;
        });
        app.tick(1.0 / 144.0);
        let t = *app.world.get::<Transform>(e).unwrap();
        assert_eq!(drawn(&app, e), t.translation);
        // Written between frames (a test, the editor): the next step blends
        // from there, not from the old position.
        let mut app3 = self::app().0;
        app3.run_frames(10, 1.0 / 60.0);
        let e3 = app3.world.first_with::<MoverTag>().unwrap();
        app3.world
            .component_mut::<Transform>(e3)
            .unwrap()
            .translation
            .z = 3.0;
        app3.tick(1.0 / 60.0);
        assert_eq!(
            interpolated_transform(&app3.world, e3)
                .unwrap()
                .translation
                .z,
            3.0
        );
        assert_eq!(drawn(&app3, e3).z, 3.0);
        // reset() snaps a move made in FixedUpdate.
        let mut app2 = self::app().0;
        app2.run_frames(10, 1.0 / 144.0);
        let e2 = app2.world.first_with::<MoverTag>().unwrap();
        Interpolated::reset(&mut app2.world, e2);
        app2.tick(0.0);
        assert_eq!(
            drawn(&app2, e2),
            app2.world.get::<Transform>(e2).unwrap().translation
        );
    }

    #[test]
    fn removing_interpolated_restores_the_simulated_transform() {
        let (mut app, e, child) = app();
        app.run_frames(9, 1.0 / 144.0);
        assert_ne!(
            drawn(&app, e),
            app.world.get::<Transform>(e).unwrap().translation
        );
        app.world.remove_one::<Interpolated>(e).unwrap();
        app.tick(0.0);
        let sim = app.world.get::<Transform>(e).unwrap().translation;
        assert_eq!(drawn(&app, e), sim);
        assert_eq!(drawn(&app, child), sim + Vec3::Y);
        assert!(app.world.get::<InterpolationState>(e).is_none());
    }
}

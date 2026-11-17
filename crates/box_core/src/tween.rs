//! [`Tween`]: animate any reflected number, vector, quaternion or colour
//! field of an entity's components by path.
//!
//! ```
//! use box_core::prelude::*;
//! use box_core::tween::TweenPlugin;
//!
//! let mut app = App::new();
//! app.add_plugin(&TweenPlugin);
//! let e = app.world.spawn((
//!     Transform::default(),
//!     Tween::to("Transform.translation.y", 2.0).over(0.5).ease(Ease::QuadInOut),
//! ));
//! app.run_frames(30, 1.0 / 60.0);
//! assert!((app.world.get::<Transform>(e).unwrap().translation.y - 2.0).abs() < 1e-5);
//! assert!(!app.world.has::<Tween>(e), "a finished one-shot tween is removed");
//! ```
//!
//! The path is parsed and resolved once, on the first frame the tween runs
//! (its start value is read then); each later frame writes the field
//! through the component's reflection accessor without parsing or registry
//! lookups. Tweens advance by [`Time::delta`](crate::time::Time::delta) in
//! `Update`, so time scale and pause apply. One `Tween` per entity; a
//! finished [`Repeat::Once`] tween removes itself (react with
//! `app.on_remove::<Tween>`). A path that does not resolve, or a value of
//! the wrong kind, logs a warning and removes the tween.

use crate::pmath;
use std::sync::Arc;

use bevy_reflect::{PartialReflect, Reflect, ReflectPath};
use glam::{Quat, Vec2, Vec3, Vec4};

use crate::app::{App, Plugin, Stage};
use crate::color::Color;
use crate::reflect::FieldPath;
use crate::time::Time;
use crate::world::{Entity, World};

/// A value a [`Tween`] animates towards. Converts from `f32`, `f64`, the
/// glam vectors, `Quat` and [`Color`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TweenValue {
    F32(f32),
    F64(f64),
    Vec2(Vec2),
    Vec3(Vec3),
    Vec4(Vec4),
    /// Interpolated with `slerp`.
    Quat(Quat),
    /// Interpolated per linear channel.
    Color(Color),
}

macro_rules! from_value {
    ($($t:ty => $v:ident),*) => {$(
        impl From<$t> for TweenValue {
            fn from(v: $t) -> Self {
                Self::$v(v)
            }
        }
    )*};
}
from_value!(f32 => F32, f64 => F64, Vec2 => Vec2, Vec3 => Vec3, Vec4 => Vec4, Quat => Quat, Color => Color);

impl TweenValue {
    /// Read a field of a supported type.
    fn read(field: &dyn PartialReflect) -> Option<Self> {
        let r = field.try_as_reflect()?;
        Some(if let Some(v) = r.downcast_ref::<f32>() {
            Self::F32(*v)
        } else if let Some(v) = r.downcast_ref::<f64>() {
            Self::F64(*v)
        } else if let Some(v) = r.downcast_ref::<Vec2>() {
            Self::Vec2(*v)
        } else if let Some(v) = r.downcast_ref::<Vec3>() {
            Self::Vec3(*v)
        } else if let Some(v) = r.downcast_ref::<Vec4>() {
            Self::Vec4(*v)
        } else if let Some(v) = r.downcast_ref::<Quat>() {
            Self::Quat(*v)
        } else {
            Self::Color(*r.downcast_ref::<Color>()?)
        })
    }

    /// Write into a field of the same kind. False on a kind mismatch.
    fn write(self, field: &mut dyn PartialReflect) -> bool {
        let Some(r) = field.try_as_reflect_mut() else {
            return false;
        };
        fn put<T: Reflect>(r: &mut dyn Reflect, v: T) -> bool {
            r.downcast_mut::<T>().map(|f| *f = v).is_some()
        }
        match self {
            Self::F32(v) => put(r, v),
            Self::F64(v) => put(r, v),
            Self::Vec2(v) => put(r, v),
            Self::Vec3(v) => put(r, v),
            Self::Vec4(v) => put(r, v),
            Self::Quat(v) => put(r, v),
            Self::Color(v) => put(r, v),
        }
    }

    /// `self` converted to the kind of `like` (numbers convert between f32
    /// and f64), or `None`.
    fn as_kind_of(self, like: Self) -> Option<Self> {
        Some(match (self, like) {
            (Self::F32(v), Self::F64(_)) => Self::F64(f64::from(v)),
            (Self::F64(v), Self::F32(_)) => Self::F32(v as f32),
            (a, b) if std::mem::discriminant(&a) == std::mem::discriminant(&b) => a,
            _ => return None,
        })
    }

    /// `self + delta` (for a quaternion: `delta * self`).
    fn plus(self, delta: Self) -> Option<Self> {
        Some(match (self, delta.as_kind_of(self)?) {
            (Self::F32(a), Self::F32(b)) => Self::F32(a + b),
            (Self::F64(a), Self::F64(b)) => Self::F64(a + b),
            (Self::Vec2(a), Self::Vec2(b)) => Self::Vec2(a + b),
            (Self::Vec3(a), Self::Vec3(b)) => Self::Vec3(a + b),
            (Self::Vec4(a), Self::Vec4(b)) => Self::Vec4(a + b),
            (Self::Quat(a), Self::Quat(b)) => Self::Quat(b * a),
            (Self::Color(a), Self::Color(b)) => Self::Color(Color::linear_rgba(
                a.r + b.r,
                a.g + b.g,
                a.b + b.b,
                a.a + b.a,
            )),
            _ => return None,
        })
    }

    /// `a` to `b` at `t` (same kinds).
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        match (a, b) {
            (Self::F32(a), Self::F32(b)) => Self::F32(a + (b - a) * t),
            (Self::F64(a), Self::F64(b)) => Self::F64(a + (b - a) * f64::from(t)),
            (Self::Vec2(a), Self::Vec2(b)) => Self::Vec2(a.lerp(b, t)),
            (Self::Vec3(a), Self::Vec3(b)) => Self::Vec3(a.lerp(b, t)),
            (Self::Vec4(a), Self::Vec4(b)) => Self::Vec4(a.lerp(b, t)),
            (Self::Quat(a), Self::Quat(b)) => Self::Quat(a.slerp(b, t)),
            (Self::Color(a), Self::Color(b)) => {
                let (a, b) = (Vec4::new(a.r, a.g, a.b, a.a), Vec4::new(b.r, b.g, b.b, b.a));
                let c = a.lerp(b, t);
                Self::Color(Color::linear_rgba(c.x, c.y, c.z, c.w))
            }
            (a, _) => a,
        }
    }
}

/// Easing curves: how progress (0..1 in time) maps to progress in value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ease {
    #[default]
    Linear,
    QuadIn,
    QuadOut,
    QuadInOut,
    CubicIn,
    CubicOut,
    CubicInOut,
    SineIn,
    SineOut,
    /// Half a cosine wave: with [`Repeat::PingPong`] the value moves exactly
    /// like a sine wave (bobbing, breathing, swaying).
    SineInOut,
}

impl Ease {
    /// The eased value of `t` (clamped to 0..1).
    pub fn apply(self, t: f32) -> f32 {
        use std::f32::consts::PI;
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::QuadIn => t * t,
            Self::QuadOut => t * (2.0 - t),
            Self::QuadInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
            Self::CubicIn => t * t * t,
            Self::CubicOut => 1.0 - (1.0 - t).powi(3),
            Self::CubicInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
            Self::SineIn => 1.0 - pmath::cos(t * PI / 2.0),
            Self::SineOut => pmath::sin(t * PI / 2.0),
            Self::SineInOut => (1.0 - pmath::cos(t * PI)) / 2.0,
        }
    }
}

/// What a [`Tween`] does when it reaches the end.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Repeat {
    /// Stop at the end value and remove the tween.
    #[default]
    Once,
    /// Jump back to the start and play again, forever.
    Loop,
    /// Play backwards to the start, then forwards again, forever.
    PingPong,
}

/// Component: animate one reflected field of this entity (see the [module
/// docs](self)). Build with [`Tween::to`] or [`Tween::by`], then
/// [`over`](Self::over), [`ease`](Self::ease), [`repeat`](Self::repeat),
/// [`from`](Self::from) and [`offset`](Self::offset).
#[derive(Clone, Debug)]
pub struct Tween {
    path: String,
    target: TweenValue,
    relative: bool,
    from: Option<TweenValue>,
    duration: f32,
    ease: Ease,
    repeat: Repeat,
    elapsed: f32,
    resolved: Option<Arc<Resolved>>,
}

/// A tween's path resolved against the world: the component accessor, the
/// pre-parsed field path and the end points.
#[derive(Debug)]
struct Resolved {
    get_mut: for<'w> fn(&'w mut World, Entity) -> Option<&'w mut dyn Reflect>,
    path: FieldPath,
    start: TweenValue,
    end: TweenValue,
}

impl Tween {
    fn new(path: impl Into<String>, target: TweenValue, relative: bool) -> Self {
        Self {
            path: path.into(),
            target,
            relative,
            from: None,
            duration: 1.0,
            ease: Ease::Linear,
            repeat: Repeat::Once,
            elapsed: 0.0,
            resolved: None,
        }
    }

    /// Animate the field at `path` (`"Transform.translation.y"`,
    /// `"StandardMaterial.base_color"`, `"Light.intensity"`) from its value
    /// when the tween starts to `value`. One second, linear, once, unless
    /// set otherwise.
    pub fn to(path: impl Into<String>, value: impl Into<TweenValue>) -> Self {
        Self::new(path, value.into(), false)
    }

    /// Animate the field at `path` by `delta` from its value when the tween
    /// starts (for a quaternion: rotate by `delta`).
    pub fn by(path: impl Into<String>, delta: impl Into<TweenValue>) -> Self {
        Self::new(path, delta.into(), true)
    }

    /// Start from `value` instead of the field's current value.
    pub fn from(mut self, value: impl Into<TweenValue>) -> Self {
        self.from = Some(value.into());
        self
    }

    /// Seconds from start to end (one way, for [`Repeat::PingPong`]).
    pub fn over(mut self, seconds: f32) -> Self {
        self.duration = seconds;
        self
    }

    pub fn ease(mut self, ease: Ease) -> Self {
        self.ease = ease;
        self
    }

    pub fn repeat(mut self, repeat: Repeat) -> Self {
        self.repeat = repeat;
        self
    }

    /// Start this many seconds into the animation: copies of one tween with
    /// different offsets move out of step.
    pub fn offset(mut self, seconds: f32) -> Self {
        self.elapsed = seconds.max(0.0);
        self
    }

    /// The path this tween animates.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Seconds played (wrapped to one cycle for repeating tweens).
    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// Progress through the current cycle, in time, 0..1.
    pub fn fraction(&self) -> f32 {
        if self.duration <= 0.0 {
            return 1.0;
        }
        let t = self.elapsed / self.duration;
        match self.repeat {
            Repeat::Once => t.min(1.0),
            Repeat::Loop => t.fract(),
            Repeat::PingPong => {
                let c = t % 2.0;
                if c <= 1.0 { c } else { 2.0 - c }
            }
        }
    }

    /// Advance by `dt`; true when a [`Repeat::Once`] tween reached its end.
    fn advance(&mut self, dt: f32) -> bool {
        if self.duration <= 0.0 {
            self.elapsed = 0.0;
            return self.repeat == Repeat::Once;
        }
        self.elapsed += dt.max(0.0);
        // Keep repeating tweens within one cycle, so precision never decays.
        match self.repeat {
            Repeat::Once => return self.elapsed >= self.duration,
            Repeat::Loop if self.elapsed >= self.duration => self.elapsed %= self.duration,
            Repeat::PingPong if self.elapsed >= 2.0 * self.duration => {
                self.elapsed %= 2.0 * self.duration
            }
            _ => {}
        }
        false
    }

    /// Resolve the path and the end points against `world`'s current
    /// values.
    fn resolve(&self, world: &mut World, e: Entity) -> Result<Resolved, String> {
        let path = world
            .field_path(&self.path)
            .map_err(|err| err.to_string())?;
        if path.is_resource() || path.is_whole() {
            return Err("tweens animate a component field (`Component.field`)".into());
        }
        let ty = world
            .registry()
            .component(path.target())
            .filter(|t| t.name() != "Uid")
            .ok_or_else(|| format!("`{}` is not an animatable component", path.target()))?;
        let get_mut = ty.data.get_mut;
        let root =
            get_mut(world, e).ok_or_else(|| format!("the entity has no `{}`", path.target()))?;
        let field = path
            .parsed()
            .reflect_element_mut(root.as_partial_reflect_mut())
            .map_err(|err| err.to_string())?;
        let current = TweenValue::read(field).ok_or_else(|| {
            format!(
                "`{}` is a {}; tweens animate numbers, vectors, quaternions and colours",
                self.path,
                field.reflect_type_path()
            )
        })?;
        let mismatch = |v: TweenValue| format!("{v:?} does not fit `{}` ({current:?})", self.path);
        let start = match self.from {
            Some(v) => v.as_kind_of(current).ok_or_else(|| mismatch(v))?,
            None => current,
        };
        let end = if self.relative {
            start
                .plus(self.target)
                .ok_or_else(|| mismatch(self.target))?
        } else {
            self.target
                .as_kind_of(current)
                .ok_or_else(|| mismatch(self.target))?
        };
        Ok(Resolved {
            get_mut,
            path,
            start,
            end,
        })
    }
}

/// Adds [`animate_tweens`] to `Update`. `box_engine` apps include it.
pub struct TweenPlugin;

impl Plugin for TweenPlugin {
    fn build(&self, app: &mut App) {
        app.add_system(Stage::Update, animate_tweens);
    }
}

/// Advance every [`Tween`] by [`Time::delta`] and write its field. Resolves
/// new tweens first; removes finished and broken ones.
pub fn animate_tweens(world: &mut World) {
    let dt = world.get_resource::<Time>().map_or(0.0, |t| t.delta);
    // New tweens: resolve the path and read the start value, once.
    let new: Vec<Entity> = world
        .query::<(Entity, &Tween)>()
        .iter()
        .filter(|(_, t)| t.resolved.is_none())
        .map(|(e, _)| e)
        .collect();
    let mut remove: Vec<Entity> = Vec::new();
    for e in new {
        let Some(tween) = world.get::<Tween>(e).map(|t| Tween::clone(&t)) else {
            continue;
        };
        match tween.resolve(world, e) {
            Ok(r) => {
                if let Some(t) = world.component_mut::<Tween>(e) {
                    t.resolved = Some(Arc::new(r));
                }
            }
            Err(message) => {
                tracing::warn!("tween on {e:?} removed: {message}");
                remove.push(e);
            }
        }
    }
    let mut writes: Vec<(Entity, Arc<Resolved>, TweenValue)> = Vec::new();
    for (e, tween) in world.query_mut::<(Entity, &mut Tween)>() {
        let Some(r) = tween.resolved.clone() else {
            continue;
        };
        let done = tween.advance(dt);
        let value = TweenValue::lerp(r.start, r.end, tween.ease.apply(tween.fraction()));
        writes.push((e, r, value));
        if done {
            remove.push(e);
        }
    }
    for (e, r, value) in writes {
        let written = (r.get_mut)(world, e).is_some_and(|root| {
            r.path
                .parsed()
                .reflect_element_mut(root.as_partial_reflect_mut())
                .is_ok_and(|field| value.write(field))
        });
        if !written {
            tracing::warn!("tween on {e:?} removed: `{}` is gone", r.path);
            remove.push(e);
        }
    }
    for e in remove {
        let _ = world.remove_one::<Tween>(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::StandardMaterial;
    use crate::transform::Transform;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugin(&TweenPlugin);
        app
    }

    fn y(app: &App, e: Entity) -> f32 {
        app.world.get::<Transform>(e).unwrap().translation.y
    }

    #[test]
    fn eases_hit_their_end_points() {
        for ease in [
            Ease::Linear,
            Ease::QuadIn,
            Ease::QuadOut,
            Ease::QuadInOut,
            Ease::CubicIn,
            Ease::CubicOut,
            Ease::CubicInOut,
            Ease::SineIn,
            Ease::SineOut,
            Ease::SineInOut,
        ] {
            assert!(ease.apply(0.0).abs() < 1e-6, "{ease:?}");
            assert!((ease.apply(1.0) - 1.0).abs() < 1e-6, "{ease:?}");
            if matches!(
                ease,
                Ease::QuadInOut | Ease::CubicInOut | Ease::SineInOut | Ease::Linear
            ) {
                assert!((ease.apply(0.5) - 0.5).abs() < 1e-6, "{ease:?}");
            }
        }
        assert_eq!(Ease::QuadIn.apply(0.5), 0.25);
    }

    #[test]
    fn once_reaches_the_target_and_removes_itself() {
        let mut app = app();
        let e = app.world.spawn((
            Transform::from_xyz(0.0, 1.0, 0.0),
            Tween::to("Transform.translation.y", 3.0).over(1.0),
        ));
        app.run_frames(30, 1.0 / 60.0);
        assert!((y(&app, e) - 2.0).abs() < 1e-4, "halfway: {}", y(&app, e));
        app.run_frames(31, 1.0 / 60.0);
        assert_eq!(y(&app, e), 3.0);
        assert!(!app.world.has::<Tween>(e));
    }

    #[test]
    fn ping_pong_by_with_offset_and_time_scale() {
        let mut app = app();
        let e = app.world.spawn((
            Transform::default(),
            Tween::by("Transform.translation.y", 2.0)
                .over(0.5)
                .repeat(Repeat::PingPong)
                .offset(0.25),
        ));
        // Resolved on the first frame, which already plays one frame.
        app.run_frames(15, 1.0 / 60.0); // 0.5 s played: at the top
        assert!((y(&app, e) - 2.0).abs() < 1e-3, "{}", y(&app, e));
        // Half speed: 0.5 real seconds play 0.25 s.
        app.world.resource_mut::<Time>().scale = 0.5;
        app.run_frames(30, 1.0 / 60.0); // 0.75 s played: halfway back
        assert!((y(&app, e) - 1.0).abs() < 1e-3, "{}", y(&app, e));
        // Paused: nothing moves.
        app.world.resource_mut::<Time>().paused = true;
        app.run_frames(7, 1.0 / 60.0);
        assert!((y(&app, e) - 1.0).abs() < 1e-3, "{}", y(&app, e));
        app.world.resource_mut::<Time>().paused = false;
        app.world.resource_mut::<Time>().scale = 1.0;
        app.run_frames(15, 1.0 / 60.0); // 1.0 s: back at the start
        assert!(y(&app, e).abs() < 1e-3, "{}", y(&app, e));
        assert!(app.world.has::<Tween>(e), "repeating tweens stay");
        assert!(app.world.get::<Tween>(e).unwrap().elapsed() <= 1.0);
    }

    #[test]
    fn colours_quaternions_and_f64_conversion() {
        let mut app = app();
        let e = app.world.spawn((
            Transform::default(),
            StandardMaterial::default(),
            Tween::to("StandardMaterial.base_color", Color::BLACK).over(0.5),
        ));
        let q = app.world.spawn((
            Transform::default(),
            Tween::to(
                "Transform.rotation",
                Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            )
            .from(Quat::IDENTITY)
            .over(1.0)
            .ease(Ease::SineInOut),
        ));
        app.run_frames(61, 1.0 / 60.0);
        let c = app.world.get::<StandardMaterial>(e).unwrap().base_color;
        assert_eq!((c.r, c.g, c.b, c.a), (0.0, 0.0, 0.0, 1.0));
        let r = app.world.get::<Transform>(q).unwrap().rotation;
        let target = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        assert!(r.dot(target).abs() > 1.0 - 1e-6, "{r}");
        assert!(!app.world.has::<Tween>(q));
    }

    #[test]
    fn bad_paths_and_kinds_remove_the_tween() {
        let mut app = app();
        let bad = [
            Tween::to("Transform.nope", 1.0),
            Tween::to("Nope.x", 1.0),
            Tween::to("Transform.translation", 1.0), // f32 into a Vec3
            Tween::to("Transform", 1.0),
            Tween::to("StandardMaterial.roughness", 1.0), // the entity has none
        ];
        let es: Vec<Entity> = bad
            .into_iter()
            .map(|t| app.world.spawn((Transform::default(), t)))
            .collect();
        app.update();
        for e in es {
            assert!(!app.world.has::<Tween>(e));
            assert_eq!(
                *app.world.get::<Transform>(e).unwrap(),
                Transform::default()
            );
        }
    }
}

//! Input mapping demo: move a cube with the `move` action (WASD, arrows,
//! left stick or d-pad), jump with `jump` (Space or South), look around with
//! the locked mouse or the right stick.
//!
//! Click (or press Enter / Start) to lock the cursor; Esc unlocks it, and Esc
//! with a free cursor quits. Movement and jumping run in `FixedUpdate`, the
//! camera in `Update`.
//!
//! `--simulate` feeds scripted keyboard and gamepad input (W held, left stick
//! right, right stick turning the camera, a jump at frame 10), so a headless
//! run shows the actions working:
//!
//! `cargo run --example input_demo -- --simulate --frames 40 --capture target/captures/input_demo.png`

mod common;

use box_engine::prelude::*;
use common::{Sun, TimeOfDay, set_time_of_day};

const SPEED: f32 = 5.0;
const JUMP_SPEED: f32 = 6.0;
const GRAVITY: f32 = 18.0;
const MOUSE_SENSITIVITY: f32 = 0.003;
const STICK_TURN_SPEED: f32 = 2.5;

/// The player cube and its vertical velocity.
struct Player {
    velocity_y: f32,
}

/// Third-person camera angles around the player.
struct Follow {
    yaw: f32,
    pitch: f32,
}

fn actions() -> ActionMap {
    ActionMap::new()
        .axis2(
            "move",
            [
                Axis2Binding::wasd(),
                Axis2Binding::arrows(),
                Axis2Binding::dpad(),
                Axis2Binding::Stick(GamepadStick::Left),
            ],
        )
        .axis2("look", [Axis2Binding::Stick(GamepadStick::Right)])
        .button(
            "jump",
            [
                Binding::Key(KeyCode::Space),
                Binding::Gamepad(GamepadButton::South),
            ],
        )
        .button(
            "lock_cursor",
            [
                Binding::Mouse(MouseButton::Left),
                Binding::Key(KeyCode::Enter),
                Binding::Gamepad(GamepadButton::Start),
            ],
        )
        .button("back", [Binding::Key(KeyCode::Escape)])
}

fn main() -> anyhow::Result<()> {
    let mut app = box_engine::default_app();
    app.world.insert_resource(actions());
    build_scene(&mut app.world);
    app.add_system(Stage::FixedUpdate, move_player);
    app.add_system(Stage::Update, cursor_and_quit);
    app.add_system(Stage::Update, follow_camera);
    if std::env::args().any(|a| a == "--simulate") {
        // PostUpdate input is seen by the next frame, as runner input would be.
        app.add_system(Stage::PostUpdate, simulate_input);
    }
    box_engine::run(app, RunOptions::new("box-engine input demo"))
}

fn build_scene(world: &mut World) {
    let mut material = |color: Color| {
        world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                base_color: color,
                roughness: 0.85,
                metallic: 0.0,
                emissive: Color::BLACK,
                ..Default::default()
            })
    };
    let ground_mat = material(Color::hex(0x8fa677));
    let player_mat = material(Color::hex(0xd8743f));
    let post_mat = material(Color::hex(0xb9b0a2));
    let meshes = world.resource_mut::<Assets<MeshData>>();
    let ground = meshes.add(MeshData::plane(80.0, 80.0));
    let cube = meshes.add(MeshData::cuboid(Vec3::ONE));
    let post = meshes.add(MeshData::cylinder(0.3, 2.0, 16));

    let render = |mesh, material| MeshRenderer {
        mesh,
        materials: vec![MaterialHandle::from(material)],
        cast_shadows: true,
    };
    world.spawn((Transform::default(), render(ground, ground_mat)));
    world.spawn((
        Transform::from_xyz(0.0, 0.5, 0.0),
        render(cube, player_mat),
        Player { velocity_y: 0.0 },
    ));
    // A ring of posts for a sense of motion.
    for i in 0..12 {
        let a = i as f32 / 12.0 * std::f32::consts::TAU;
        world.spawn((
            Transform::from_xyz(a.cos() * 8.0, 1.0, a.sin() * 8.0),
            render(post, post_mat),
        ));
    }
    world.spawn((Transform::default(), DirectionalLight::default(), Sun));
    world.spawn((
        Transform::default(),
        Camera::default(),
        Follow {
            yaw: 0.0,
            pitch: 0.35,
        },
    ));
    set_time_of_day(world, TimeOfDay::Day);
}

/// Camera-relative movement and jumping, at the fixed rate.
fn move_player(world: &mut World) {
    let dt = world.resource::<FixedTime>().step;
    let (movement, jump) = {
        let a = world.resource::<Actions>();
        (a.axis2("move"), a.pressed("jump"))
    };
    let yaw = world
        .query::<&Follow>()
        .iter()
        .next()
        .map_or(0.0, |f| f.yaw);
    // +Y on the action is forward (-Z) when the camera looks along -Z.
    let rot = Quat::from_rotation_y(yaw);
    let dir = rot * Vec3::new(movement.x, 0.0, -movement.y);
    for (t, p) in world.query::<(&mut Transform, &mut Player)>().iter() {
        t.translation += dir * SPEED * dt;
        let grounded = t.translation.y <= 0.5 + 1e-4;
        if jump && grounded {
            p.velocity_y = JUMP_SPEED;
        }
        p.velocity_y -= GRAVITY * dt;
        t.translation.y = (t.translation.y + p.velocity_y * dt).max(0.5);
        if t.translation.y <= 0.5 {
            p.velocity_y = 0.0;
        }
    }
}

/// Lock the cursor on click/Enter/Start, unlock on Esc, quit on Esc when free.
fn cursor_and_quit(world: &mut World) {
    let (lock, back) = {
        let a = world.resource::<Actions>();
        (a.pressed("lock_cursor"), a.pressed("back"))
    };
    let cursor = world.resource_mut::<CursorOptions>();
    if lock {
        *cursor = CursorOptions {
            active_grab: cursor.active_grab,
            ..CursorOptions::locked()
        };
    } else if back {
        if cursor.grab == CursorGrab::None {
            world.resource_mut::<AppExit>().0 = true;
        } else {
            *cursor = CursorOptions {
                active_grab: cursor.active_grab,
                ..CursorOptions::free()
            };
        }
    }
}

/// Mouse look (only while the cursor is actually grabbed) plus right-stick
/// look, then place the camera behind the player.
fn follow_camera(world: &mut World) {
    let dt = world.resource::<Time>().delta;
    let look = world.resource::<Actions>().axis2("look");
    let grabbed = world.resource::<CursorOptions>().active_grab != CursorGrab::None;
    let mouse = if grabbed {
        world.resource::<Input>().mouse_delta
    } else {
        Vec2::ZERO
    };
    let target = world
        .query::<(&Transform, &Player)>()
        .iter()
        .next()
        // Follow on the ground plane only, so jumps are visible.
        .map_or(Vec3::ZERO, |(t, _)| {
            t.translation * Vec3::new(1.0, 0.0, 1.0)
        })
        + Vec3::Y * 0.5;
    for (t, f) in world.query::<(&mut Transform, &mut Follow)>().iter() {
        f.yaw -= mouse.x * MOUSE_SENSITIVITY + look.x * STICK_TURN_SPEED * dt;
        f.pitch = (f.pitch + mouse.y * MOUSE_SENSITIVITY - look.y * STICK_TURN_SPEED * dt)
            .clamp(-0.2, 1.3);
        let offset =
            Quat::from_euler(EulerRot::YXZ, f.yaw, -f.pitch, 0.0) * Vec3::new(0.0, 0.0, 9.0);
        *t = Transform::from_translation(target + offset).looking_at(target, Vec3::Y);
    }
}

/// Scripted input for headless runs: keyboard and a virtual gamepad.
fn simulate_input(world: &mut World) {
    let frame = world.resource::<Time>().frame;
    let pad = GamepadId(0);
    let input = world.resource_mut::<Input>();
    if frame == 1 {
        input.connect_gamepad(pad, "simulated");
        input.press_key(KeyCode::W);
        input.set_gamepad_axis(pad, GamepadAxis::LeftStickX, 0.8);
        input.set_gamepad_axis(pad, GamepadAxis::RightStickX, 0.4);
    }
    if frame == 10 {
        input.press_gamepad_button(pad, GamepadButton::South);
    }
    if frame == 12 {
        input.release_gamepad_button(pad, GamepadButton::South);
    }
}

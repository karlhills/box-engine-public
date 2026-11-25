//! Interactions (docs/INTERACTIONS.md): `examples/ember_bridge`, a data
//! project whose puzzle is configured with `Interactable` components only,
//! played headlessly by the stock player with injected actions
//! (`Actions::inject`).
//!
//! - `ember_bridge_plays_through`: a brazier refuses without an ember,
//!   an ember is picked up by walking into it, the lit brazier spends it,
//!   starts its flame (inside the prefab), lights its glow and reveals a
//!   bridge the player then walks across; the second brazier reveals the
//!   other bridge. A save made after the first brazier loads back with the
//!   flame lit and the bridge solid. Captures go to
//!   `target/captures/interact/` (prompt, blocked prompt, revealed bridge).
//! - `interact_action_reaches_the_game_through_the_control_socket`: the
//!   live control socket's `send_input` presses `interact`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use box_engine::box_assets::AssetServer;
use box_engine::box_core::action::InjectedInput;
use box_engine::box_physics::Collidable;
use box_engine::box_render::{ParticleEmitter, PointLight};
use box_engine::interact::{GameVars, InteractFocus, Interactable};
use box_engine::player::{PlayerOptions, build_app};
use box_engine::prelude::*;
use box_render::RenderError;

const DT: f32 = 1.0 / 60.0;

fn project() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/ember_bridge")
}

fn captures() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures/interact");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn save_dir() -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    std::env::temp_dir().join(format!(
        "box_interact_saves_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

struct Play {
    app: App,
    renderer: Option<Renderer>,
}

impl Play {
    fn new(w: u32, h: u32) -> Self {
        let options = PlayerOptions {
            project: Some(project()),
            watch: false,
            ..Default::default()
        };
        let (mut app, _) = build_app(&Game::new(), &options).unwrap();
        let dir = save_dir();
        let _ = std::fs::remove_dir_all(&dir);
        app.world.resource_mut::<SaveConfig>().dir = Some(dir);
        app.world.insert_resource(WindowSize {
            width: w,
            height: h,
            scale_factor_milli: 1000,
        });
        let renderer = if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
            None
        } else {
            match Renderer::new_headless(w, h) {
                Ok(r) => Some(r),
                Err(RenderError::NoAdapter) => None,
                Err(e) => panic!("renderer: {e}"),
            }
        };
        let mut play = Self { app, renderer };
        play.steps(30);
        play
    }

    fn world(&mut self) -> &mut World {
        &mut self.app.world
    }

    fn step(&mut self) {
        if let Some(s) = self.app.world.get_resource::<AssetServer>().cloned() {
            s.update_until_idle(&mut self.app.world, Duration::from_millis(50));
        }
        self.app.run_frames(1, DT);
        self.app.world.resource_mut::<Input>().end_frame();
    }

    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    fn capture(&mut self, name: &str) {
        let Some(r) = &mut self.renderer else { return };
        let img = r.capture(&mut self.app.world).expect("capture");
        img.save(captures().join(name)).unwrap();
        assert_eq!(
            r.stats().validation_errors,
            0,
            "{name}: GPU validation errors"
        );
    }

    fn entity(&mut self, name: &str) -> Entity {
        self.world()
            .find_by_name(name)
            .unwrap_or_else(|| panic!("no entity named {name}"))
    }

    fn player_pos(&mut self) -> Vec3 {
        let p = self.entity("Player");
        self.world()
            .get::<GlobalTransform>(p)
            .unwrap()
            .0
            .translation
            .into()
    }

    /// Walks toward `target` (xz) with the `move` action until within
    /// `tolerance`; the follow camera looks along -Z, so +Y on the stick is
    /// world -Z.
    fn walk_to(&mut self, target: Vec3, tolerance: f32, max_frames: usize) {
        for _ in 0..max_frames {
            let at = self.player_pos();
            let d = Vec2::new(target.x - at.x, target.z - at.z);
            if d.length() <= tolerance {
                self.steps(10);
                return;
            }
            let wish = Vec2::new(d.x, -d.y).normalize() * if d.length() < 1.0 { 0.5 } else { 1.0 };
            self.world()
                .resource_mut::<Actions>()
                .inject("move", InjectedInput::axis(wish, 1));
            self.step();
        }
        panic!("did not reach {target} (at {})", self.player_pos());
    }

    fn press_interact(&mut self) {
        self.world()
            .resource_mut::<Actions>()
            .inject("interact", InjectedInput::press(2));
        self.steps(3);
    }

    fn embers(&mut self) -> f64 {
        self.world().resource::<GameVars>().get("embers")
    }

    fn focus(&mut self) -> InteractFocus {
        self.world().resource::<InteractFocus>().clone()
    }

    fn flame_playing(&mut self, brazier: &str) -> bool {
        let b = self.entity(brazier);
        let flame = box_engine::interact::resolve(
            self.world(),
            b,
            &box_engine::interact::Target::Child("Flame".into()),
        )
        .expect("the prefab's Flame");
        self.world().get::<ParticleEmitter>(flame).unwrap().playing
    }

    fn glow(&mut self, brazier: &str) -> f32 {
        let b = self.entity(brazier);
        let glow = box_engine::interact::resolve(
            self.world(),
            b,
            &box_engine::interact::Target::Child("Glow".into()),
        )
        .expect("the prefab's Glow");
        self.world().get::<PointLight>(glow).unwrap().intensity
    }

    /// The path of the material in the brazier mesh's first slot.
    fn coals(&mut self, brazier: &str) -> String {
        let b = self.entity(brazier);
        let mesh = box_engine::interact::resolve(
            self.world(),
            b,
            &box_engine::interact::Target::Child("Brazier".into()),
        )
        .expect("the prefab's Brazier");
        let m = self.world().get::<MeshAsset>(mesh).unwrap();
        m.materials[0].to_string()
    }

    fn bridge_open(&mut self, bridge: &str) -> bool {
        let b = self.entity(bridge);
        let w = self.world();
        let visible = w.get::<Visible>(b).is_none_or(|v| v.0);
        let solid = w.get::<Collidable>(b).is_none_or(|c| c.0);
        assert_eq!(visible, solid, "{bridge}: visible and solid together");
        visible
    }
}

const BRAZIER_WEST: Vec3 = Vec3::new(2.5, 0.0, -0.8);
const EMBER_WEST: Vec3 = Vec3::new(-4.0, 0.9, 3.0);
const EMBER_MIDDLE: Vec3 = Vec3::new(4.0, 0.9, -12.0);
const BRAZIER_MIDDLE: Vec3 = Vec3::new(-2.5, 0.0, -16.8);

#[test]
fn ember_bridge_plays_through() {
    let mut p = Play::new(1280, 720);
    assert_eq!(p.embers(), 0.0);
    assert!(!p.bridge_open("Bridge West") && !p.bridge_open("Bridge East"));
    assert!(!p.flame_playing("Brazier West"));

    // Without an ember the brazier shows its blocked prompt and refuses.
    p.walk_to(BRAZIER_WEST + Vec3::new(-1.2, 0.0, 1.0), 0.3, 600);
    let f = p.focus();
    assert_eq!(f.entity, Some(p.entity("Brazier West")));
    assert!(f.blocked, "{f:?}");
    assert_eq!(f.text, "Needs an ember");
    p.capture("1_blocked.png");
    p.press_interact();
    assert!(!p.flame_playing("Brazier West"), "lit without an ember");
    assert!(!p.bridge_open("Bridge West"));

    // Walking into the ember picks it up (a Touch interactable).
    p.walk_to(EMBER_WEST, 0.5, 600);
    assert_eq!(p.embers(), 1.0);
    let ember = p.entity("Ember West");
    assert_eq!(p.world().get::<Visible>(ember).map(|v| v.0), Some(false));
    assert_eq!(p.world().get::<Interactable>(ember).unwrap().uses, 1);

    // Back at the brazier: the prompt, then E lights it and reveals the bridge.
    p.walk_to(BRAZIER_WEST + Vec3::new(-1.2, 0.0, 1.0), 0.3, 600);
    let f = p.focus();
    assert!(!f.blocked, "{f:?}");
    assert_eq!(
        (f.key.as_str(), f.text.as_str()),
        ("E", "Light the brazier")
    );
    p.capture("2_prompt.png");
    p.press_interact();
    assert_eq!(p.embers(), 0.0, "the ember is spent");
    assert!(p.flame_playing("Brazier West"));
    assert_eq!(p.glow("Brazier West"), 40.0);
    assert_eq!(
        p.coals("Brazier West"),
        "models/props.glb#Material/Ember",
        "SetField"
    );
    assert!(p.bridge_open("Bridge West"));
    assert!(
        !p.bridge_open("Bridge East"),
        "the other bridge stays hidden"
    );
    assert_eq!(p.focus().entity, None, "a used-up brazier shows no prompt");
    p.steps(40);
    p.capture("3_bridge.png");

    // A save now loads back with the flame (prefab contents) lit again.
    p.world().resource_mut::<SaveGames>().save("lit");
    p.steps(2);

    // Across the bridge: the player stays on top of it.
    p.walk_to(Vec3::new(0.0, 0.0, -1.0), 0.4, 600);
    p.walk_to(Vec3::new(0.0, 0.0, -10.0), 0.5, 900);
    assert!(p.player_pos().y > 0.5, "fell off: {}", p.player_pos());

    // The second ember and brazier open the far bridge.
    p.walk_to(EMBER_MIDDLE, 0.5, 900);
    assert_eq!(p.embers(), 1.0);
    p.walk_to(BRAZIER_MIDDLE + Vec3::new(1.2, 0.0, 1.0), 0.3, 900);
    p.press_interact();
    assert!(p.bridge_open("Bridge East"));
    p.walk_to(Vec3::new(0.0, 0.0, -17.0), 0.4, 600);
    p.walk_to(Vec3::new(0.0, 0.0, -27.0), 0.5, 900);
    assert!(p.player_pos().y > 0.5, "fell off: {}", p.player_pos());

    // Load: the first brazier is lit, its bridge solid, the second not.
    p.world().resource_mut::<SaveGames>().load("lit");
    p.steps(20);
    assert_eq!(p.embers(), 0.0);
    assert!(p.flame_playing("Brazier West"), "flame restored after load");
    assert_eq!(p.glow("Brazier West"), 40.0);
    assert_eq!(p.coals("Brazier West"), "models/props.glb#Material/Ember");
    assert_eq!(p.coals("Brazier Middle"), "materials/ash.material.json");
    assert!(p.bridge_open("Bridge West"));
    assert!(!p.bridge_open("Bridge East"));
    assert!(!p.flame_playing("Brazier Middle"));
    assert!(
        p.player_pos().z > -8.0,
        "the player is back west: {}",
        p.player_pos()
    );
}

/// The player app as a live control host: `send_input` acts on its world.
struct LiveHost<'a>(&'a mut App);

impl box_engine::control::ControlHost for LiveHost<'_> {
    fn status(&mut self) -> box_engine::control::Status {
        box_engine::control::Status {
            mode: box_engine::control::Mode::Play,
            scene: None,
            project: None,
            frame: 0,
            undo: None,
        }
    }

    fn play_world(
        &mut self,
        _method: &str,
    ) -> Result<&mut World, box_engine::control::ControlError> {
        Ok(&mut self.0.world)
    }
}

#[test]
fn interact_action_reaches_the_game_through_the_control_socket() {
    use box_engine::control::{ControlClient, ControlServer, HostKind};

    let mut p = Play::new(320, 180);
    // Stand at the brazier with an ember.
    p.world().resource_mut::<GameVars>().set("embers", 1.0);
    p.walk_to(BRAZIER_WEST + Vec3::new(-1.2, 0.0, 1.0), 0.3, 600);
    assert_eq!(p.focus().entity, Some(p.entity("Brazier West")));

    let mut server = ControlServer::bind(HostKind::Player, None).unwrap();
    let discovery = server.discovery();
    let client = std::thread::spawn(move || {
        let mut c = ControlClient::connect(discovery, Duration::from_secs(30)).unwrap();
        c.call(
            "send_input",
            serde_json::json!({ "action": "interact", "frames": 2 }),
        )
        .unwrap()
    });
    for _ in 0..2000 {
        server.poll(&mut LiveHost(&mut p.app));
        p.step();
        if client.is_finished() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let reply = client.join().unwrap();
    p.steps(3);
    assert!(
        p.flame_playing("Brazier West"),
        "send_input replied {reply}"
    );
    assert!(p.bridge_open("Bridge West"));
}

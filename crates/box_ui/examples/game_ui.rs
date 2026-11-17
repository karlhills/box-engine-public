//! In-game UI and save games (M7): a title menu (Start, Options, Quit), an
//! options screen (slider, toggles, one switching the language), a HUD
//! (health bar, coin counter in a theme font, a label over each coin), a
//! dialogue box whose lines come from a data table and a string table in
//! English and German, and a pause menu that saves and loads the game.
//!
//! Everything is navigable by gamepad (D-pad or left stick, South confirms,
//! East goes back, Start pauses) and keyboard (arrows, Enter, Escape, P).
//! Assets: `crates/box_ui/examples/assets` (theme, fonts, string tables,
//! data table). Saves go to `target/saves/game_ui/`.
//!
//! `--simulate` drives a virtual gamepad through the whole flow; captures
//! at these frames show each screen (headless runs are deterministic):
//!
//! ```text
//! cargo run --example game_ui -- --simulate --frames 18  --capture target/captures/game_ui/1_title.png
//! cargo run --example game_ui -- --simulate --frames 112 --capture target/captures/game_ui/2_options.png
//! cargo run --example game_ui -- --simulate --frames 150 --capture target/captures/game_ui/3_title_de.png
//! cargo run --example game_ui -- --simulate --frames 200 --capture target/captures/game_ui/4_dialogue.png
//! cargo run --example game_ui -- --simulate --frames 330 --capture target/captures/game_ui/5_hud.png
//! cargo run --example game_ui -- --simulate --frames 352 --capture target/captures/game_ui/6_pause.png
//! cargo run --example game_ui -- --simulate --frames 480 --capture target/captures/game_ui/7_all_coins.png
//! cargo run --example game_ui -- --simulate --frames 560 --capture target/captures/game_ui/8_loaded.png
//! ```

use box_engine::box_ui::{Align, Justify, NodeId, Size, Strings, Ui, UiEvent, UiEventKind, UiNode};
use box_engine::data::data_table;
use box_engine::prelude::*;
use box_engine::savegame::save_dir;

const SLOT: &str = "demo";
const DIALOGUE: &str = "data/dialogue.data.json";

/// The player's cube.
#[derive(Reflect, Default, Clone)]
#[reflect(Component, Default, Clone)]
struct Player;

/// A coin to collect.
#[derive(Reflect, Default, Clone)]
#[reflect(Component, Default, Clone)]
struct Coin;

/// Coins collected (a saved resource).
#[derive(Reflect, Default, Clone)]
#[reflect(Resource, Default, Clone)]
struct Score {
    coins: u32,
    total: u32,
}

/// Seconds played: plain runtime state, saved through a `SaveHooks` hook.
#[derive(Default)]
struct Playtime(f32);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Screen {
    #[default]
    Title,
    Options,
    Dialogue,
    Playing,
    Paused,
}

/// The nodes the game updates.
struct Nodes {
    title: NodeId,
    options: NodeId,
    pause: NodeId,
    hud: NodeId,
    health: NodeId,
    coins: NodeId,
    dialogue: NodeId,
    speaker: NodeId,
    line: NodeId,
    toast: NodeId,
}

#[derive(Default)]
struct State {
    screen: Screen,
    line: usize,
    toast_left: f32,
    labels: Vec<NodeId>,
}

struct Demo;

impl Plugin for Demo {
    fn register(&self, registry: &mut Registry) {
        registry.register_all::<(Player, Coin)>();
        let _ = registry.register_resource::<Score>();
    }

    fn build(&self, app: &mut App) {
        let mut map = ActionMap::new()
            .axis2(
                "move",
                [
                    Axis2Binding::wasd(),
                    Axis2Binding::Stick(GamepadStick::Left),
                ],
            )
            .button(
                "pause",
                [
                    Binding::Key(KeyCode::P),
                    Binding::Gamepad(GamepadButton::Start),
                ],
            );
        map.merge_missing(&box_engine::box_ui::default_actions());
        app.world.insert_resource(map);
        app.world.resource_or_default::<Score>();
        app.world.resource_or_default::<Playtime>();
        *app.world.resource_mut::<SaveConfig>() = SaveConfig {
            dir: Some("target/saves/game_ui".into()),
            game: "game_ui".into(),
            ..Default::default()
        };
        app.world.resource_mut::<SaveHooks>().add(
            "playtime",
            |w| serde_json::Value::from(w.resource::<Playtime>().0),
            |w, v| w.resource_mut::<Playtime>().0 = v.as_f64().unwrap_or(0.0) as f32,
        );
        build_scene(&mut app.world);
        let nodes = build_ui(&mut app.world);
        app.world.insert_resource(nodes);
        app.world.insert_resource(State::default());
        // Request the dialogue table now, so headless runs wait for it.
        let _ = data_table(&mut app.world, DIALOGUE);

        let mut cursor = EventCursor::<UiEvent>::default();
        app.add_system(Stage::Update, move |w| {
            let events: Vec<UiEvent> = cursor
                .read(w.resource::<Events<UiEvent>>())
                .cloned()
                .collect();
            for e in events {
                on_ui_event(w, &e);
            }
        });
        let mut save_cursor = EventCursor::<SaveEvent>::default();
        app.add_system(Stage::Update, move |w| {
            let events: Vec<SaveEvent> = save_cursor
                .read(w.resource::<Events<SaveEvent>>())
                .cloned()
                .collect();
            for e in events {
                on_save_event(w, &e);
            }
        });
        app.add_system(Stage::Update, pause_toggle);
        app.add_system(Stage::FixedUpdate, move_player);
        app.add_system(Stage::Update, update_hud);
    }
}

fn build_scene(world: &mut World) {
    world.spawn((
        Transform::default(),
        Primitive::plane(Vec2::new(40.0, 40.0)),
        StandardMaterial::color(Color::hex(0x7d9a6a)),
    ));
    world.spawn((
        Name::new("Player"),
        Uid::new(),
        Player,
        Transform::from_xyz(0.0, 0.5, 1.0),
        Primitive::cuboid(Vec3::ONE),
        StandardMaterial::color(Color::hex(0xd8743f)),
    ));
    world.spawn((
        Name::new("Keeper"),
        Transform::from_xyz(-1.5, 0.8, -2.5),
        Primitive::capsule(0.4, 0.4),
        StandardMaterial::color(Color::hex(0x6c7fb8)),
    ));
    world.spawn((
        Transform::default().looking_at(Vec3::new(-0.5, -1.0, -0.6), Vec3::Y),
        DirectionalLight::default(),
    ));
    world.spawn((
        Transform::from_xyz(0.5, 7.0, 10.0).looking_at(Vec3::new(0.5, 0.0, 0.0), Vec3::Y),
        Camera::default(),
    ));
}

fn build_ui(world: &mut World) -> Nodes {
    let ui = world.resource_mut::<Ui>();
    ui.set_theme("ui/game.theme.json");
    // Start / P still reaches the game while a menu is open (it closes the
    // pause menu).
    ui.settings.passthrough_actions.push("pause".into());
    // Title menu.
    let title = ui.add(
        None,
        UiNode::panel()
            .modal()
            .anchored([0.5, 0.5], [0.0, 0.0])
            .with_style(|s| s.align = Some(Align::Center))
            .children([
                UiNode::label_key("title.name").with_class("title"),
                UiNode::label_key("title.tagline").with_class("subtitle"),
                UiNode::spacer().with_height(Size::Px(16.0)),
                UiNode::button("start", "menu.start").autofocus(),
                UiNode::button("options", "menu.options"),
                UiNode::button("quit", "menu.quit"),
            ]),
    );
    // Options, from a JSON document (the format scripts will use).
    let options = ui
        .add_json(
            None,
            r#"{
              "kind": "panel", "id": "options_menu", "modal": true, "hidden": true,
              "anchor": [0.5, 0.5], "style": { "width": 640 },
              "children": [
                { "kind": "text", "class": "title", "key": "options.title" },
                { "kind": "slider", "id": "volume", "key": "options.volume",
                  "value": 0.6, "min": 0, "max": 1, "step": 0.1 },
                { "kind": "toggle", "id": "subtitles", "key": "options.subtitles" },
                { "kind": "toggle", "id": "german", "key": "options.german" },
                { "kind": "button", "id": "options_back", "action": "back", "key": "options.back" }
              ]
            }"#,
        )
        .expect("the options document parses");
    // Pause menu.
    let pause = ui.add(
        None,
        UiNode::panel()
            .modal()
            .hidden()
            .anchored([0.5, 0.5], [0.0, 0.0])
            .children([
                UiNode::label_key("menu.paused").with_class("title"),
                UiNode::button("resume", "menu.resume").autofocus(),
                UiNode::button("save", "menu.save"),
                UiNode::button("load", "menu.load"),
                UiNode::button("to_title", "menu.to_title"),
            ]),
    );
    // HUD: health bar and coin counter, top left.
    let hud = ui.add(
        None,
        UiNode::column()
            .with_class("hud")
            .hidden()
            .anchored([0.0, 0.0], [32.0, 32.0])
            .children([
                UiNode::row().children([
                    UiNode::label_key("hud.health").with_style(|s| s.min_width = Some(110.0)),
                    UiNode::progress("health", 0.8),
                ]),
                UiNode::label_key("hud.coins")
                    .with_id("coins")
                    .with_class("counter")
                    .with_arg("count", 0)
                    .with_arg("total", 0),
            ]),
    );
    // Dialogue box, bottom centre.
    let dialogue = ui.add(
        None,
        UiNode::panel()
            .with_class("dialogue")
            .modal()
            .hidden()
            .with_layer(50)
            .anchored([0.5, 1.0], [0.0, -40.0])
            .children([
                UiNode::text("").with_id("speaker").with_class("speaker"),
                UiNode::text("").with_id("line"),
                UiNode::row()
                    .with_style(|s| s.justify = Some(Justify::End))
                    .child(
                        UiNode::button("next", "dialogue.next")
                            .with_style(|s| s.min_width = Some(200.0)),
                    ),
            ]),
    );
    // A short message after saving or loading, top right.
    let toast = ui.add(
        None,
        UiNode::text("")
            .with_class("toast")
            .hidden()
            .anchored([1.0, 0.0], [-32.0, 32.0]),
    );
    let find = |ui: &Ui, id: &str| ui.find(None, id).expect("node exists");
    Nodes {
        title,
        options,
        pause,
        hud,
        health: find(ui, "health"),
        coins: find(ui, "coins"),
        dialogue,
        speaker: find(ui, "speaker"),
        line: find(ui, "line"),
        toast,
    }
}

fn show(world: &mut World, screen: Screen) {
    let n = world.resource::<Nodes>();
    let (title, options, pause, hud, dialogue) = (n.title, n.options, n.pause, n.hud, n.dialogue);
    let ui = world.resource_mut::<Ui>();
    ui.set_visible(title, screen == Screen::Title);
    ui.set_visible(options, screen == Screen::Options);
    ui.set_visible(pause, screen == Screen::Paused);
    ui.set_visible(dialogue, screen == Screen::Dialogue);
    ui.set_visible(
        hud,
        matches!(screen, Screen::Playing | Screen::Paused | Screen::Dialogue),
    );
    world.resource_mut::<Time>().paused = screen == Screen::Paused;
    world.resource_mut::<State>().screen = screen;
}

fn start_game(world: &mut World) {
    // Coins from the data table: runtime entities that save games keep.
    let coins: Vec<[f32; 3]> = data_table(world, DIALOGUE)
        .and_then(|t| t.get("coins"))
        .and_then(|c| c.as_array())
        .map(|a| a.iter().filter_map(|c| array3(&c["position"])).collect())
        .unwrap_or_default();
    let labels = std::mem::take(&mut world.resource_mut::<State>().labels);
    for l in labels {
        world.resource_mut::<Ui>().remove(l);
    }
    let old: Vec<Entity> = world
        .query::<(Entity, &Coin)>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in old {
        world.despawn(e);
    }
    let mut labels = Vec::new();
    for p in &coins {
        let uid = Uid::new();
        world.spawn((
            uid,
            Saveable,
            Coin,
            Transform::from_xyz(p[0], p[1], p[2]),
            Primitive::cylinder(0.35, 0.12),
            StandardMaterial::color(Color::hex(0xf2c14e)),
        ));
        // A world-anchored label over each coin, following it by uid.
        let label = world.resource_mut::<Ui>().add(
            None,
            UiNode::label_key("hud.coin")
                .with_class("label")
                .at_entity(uid.to_string(), Vec3::new(0.0, 0.7, 0.0))
                .anchored([0.5, 1.0], [0.0, 0.0]),
        );
        labels.push(label);
    }
    world.resource_mut::<State>().labels = labels;
    *world.resource_mut::<Score>() = Score {
        coins: 0,
        total: coins.len() as u32,
    };
    if let Some(p) = world.query::<(&mut Transform, &Player)>().iter().next() {
        p.0.translation = Vec3::new(0.0, 0.5, 1.0);
    }
    world.resource_mut::<State>().line = 0;
    show_line(world);
    show(world, Screen::Dialogue);
}

fn array3(v: &serde_json::Value) -> Option<[f32; 3]> {
    let a = v.as_array()?;
    Some([
        a.first()?.as_f64()? as f32,
        a.get(1)?.as_f64()? as f32,
        a.get(2)?.as_f64()? as f32,
    ])
}

/// Shows dialogue line `State::line`; false when there are no more.
fn show_line(world: &mut World) -> bool {
    let i = world.resource::<State>().line;
    let entry = data_table(world, DIALOGUE)
        .and_then(|t| t.get(&format!("intro.{i}")))
        .cloned();
    let Some(entry) = entry else {
        return false;
    };
    let n = world.resource::<Nodes>();
    let (speaker, line) = (n.speaker, n.line);
    let ui = world.resource_mut::<Ui>();
    ui.set_key(speaker, entry["speaker"].as_str().unwrap_or(""));
    ui.set_key(line, entry["line"].as_str().unwrap_or(""));
    true
}

fn toast(world: &mut World, key: &str) {
    let toast = world.resource::<Nodes>().toast;
    let ui = world.resource_mut::<Ui>();
    ui.set_key(toast, key);
    ui.set_arg(toast, "slot", SLOT);
    ui.set_visible(toast, true);
    world.resource_mut::<State>().toast_left = 2.0;
}

fn on_ui_event(world: &mut World, e: &UiEvent) {
    let screen = world.resource::<State>().screen;
    match (&e.kind, e.action.as_deref().or(e.id.as_deref())) {
        (UiEventKind::Click, Some("start")) => start_game(world),
        (UiEventKind::Click, Some("options")) => show(world, Screen::Options),
        (UiEventKind::Click, Some("quit")) => world.resource_mut::<AppExit>().0 = true,
        (UiEventKind::Click, Some("back")) => show(world, Screen::Title),
        (UiEventKind::Back, _) if screen == Screen::Options => show(world, Screen::Title),
        (UiEventKind::Back, _) if screen == Screen::Paused => show(world, Screen::Playing),
        (UiEventKind::Toggle(on), Some("german")) => {
            world
                .resource_mut::<Strings>()
                .set_language(if *on { "de" } else { "en" });
        }
        (UiEventKind::Change(v), Some("volume")) => info!("music volume {v:.1}"),
        (UiEventKind::Click, Some("next")) => {
            world.resource_mut::<State>().line += 1;
            if !show_line(world) {
                show(world, Screen::Playing);
            }
        }
        (UiEventKind::Click, Some("resume")) => show(world, Screen::Playing),
        (UiEventKind::Click, Some("save")) => {
            world.resource_mut::<SaveGames>().save(SLOT);
            show(world, Screen::Playing);
        }
        (UiEventKind::Click, Some("load")) => {
            world.resource_mut::<SaveGames>().load(SLOT);
            show(world, Screen::Playing);
        }
        (UiEventKind::Click, Some("to_title")) => show(world, Screen::Title),
        _ => {}
    }
}

fn on_save_event(world: &mut World, e: &SaveEvent) {
    match e {
        SaveEvent::Saved { path, .. } => {
            info!("saved {}", path.display());
            toast(world, "hud.saved");
        }
        SaveEvent::Loaded { entities, .. } => {
            let score = world.resource::<Score>();
            info!(
                "loaded {entities} entities: {} of {} coins, {:.1} s played",
                score.coins,
                score.total,
                world.resource::<Playtime>().0
            );
            toast(world, "hud.loaded");
        }
        SaveEvent::Failed { message, .. } => error!("save game: {message}"),
    }
}

/// Start / P opens the pause menu while playing and closes it again (as
/// does the menu's Back).
fn pause_toggle(world: &mut World) {
    if !world.resource::<Actions>().pressed("pause") {
        return;
    }
    match world.resource::<State>().screen {
        Screen::Playing => show(world, Screen::Paused),
        Screen::Paused => show(world, Screen::Playing),
        _ => {}
    }
}

/// Moves the player (input is captured while a menu is open) and collects
/// coins it touches.
fn move_player(world: &mut World) {
    let dt = world.resource::<Time>().delta;
    world.resource_mut::<Playtime>().0 += dt;
    if world.resource::<State>().screen != Screen::Playing {
        return;
    }
    let m = world.resource::<Actions>().axis2("move");
    let mut pos = Vec3::ZERO;
    if let Some((t, _)) = world.query::<(&mut Transform, &Player)>().iter().next() {
        t.translation += Vec3::new(m.x, 0.0, -m.y) * 4.0 * dt;
        pos = t.translation;
    }
    let touched: Vec<Entity> = world
        .query::<(Entity, &Transform, &Coin)>()
        .iter()
        .filter(|(_, t, _)| t.translation.distance(pos) < 0.9)
        .map(|(e, _, _)| e)
        .collect();
    for e in touched {
        world.despawn(e);
        world.resource_mut::<Score>().coins += 1;
    }
}

/// Keeps the HUD in step with the game (setters change nothing, and cost
/// no layout, when the values are unchanged).
fn update_hud(world: &mut World) {
    let dt = world.resource::<Time>().unscaled_delta;
    let score = world.resource::<Score>().clone();
    let elapsed = world.resource::<Playtime>().0;
    let n = world.resource::<Nodes>();
    let (coins, health, toast) = (n.coins, n.health, n.toast);
    let left = {
        let s = world.resource_mut::<State>();
        s.toast_left = (s.toast_left - dt).max(0.0);
        s.toast_left
    };
    let ui = world.resource_mut::<Ui>();
    ui.set_arg(coins, "count", score.coins);
    ui.set_arg(coins, "total", score.total);
    // Health drains slowly while playing, for something to watch.
    ui.set_value(health, (1.0 - elapsed / 120.0).clamp(0.1, 1.0));
    if left == 0.0 {
        ui.set_visible(toast, false);
    }
}

/// Scripted gamepad input (PostUpdate, seen by the next frame). Each step
/// presses a button for two frames.
fn simulate(world: &mut World) {
    let frame = world.resource::<Time>().frame;
    let pad = GamepadId(0);
    use GamepadButton::*;
    let presses: &[(u64, GamepadButton)] = &[
        (20, DPadDown),  // focus Options
        (30, South),     // open Options (focus: volume slider)
        (40, DPadRight), // volume 0.7
        (50, DPadRight), // 0.8
        (60, DPadRight), // 0.9
        (70, DPadDown),  // subtitles
        (80, South),     // on
        (90, DPadDown),  // Deutsch
        (100, South),    // German
        (115, East),     // back to the title (now in German)
        (155, DPadUp),   // Start (focus moved to Options on return)
        (165, South),    // start: the dialogue opens
        (205, South),    // next line
        (215, South),    // next line
        (225, South),    // close the dialogue
        (340, Start),    // pause
        (350, DPadDown), // Save
        (360, South),    // save
        (485, Start),    // pause (focus: Save, as last time)
        (495, DPadDown), // Load
        (505, South),    // load
    ];
    let input = world.resource_mut::<Input>();
    if frame == 1 {
        input.connect_gamepad(pad, "simulated");
    }
    for &(f, b) in presses {
        if frame == f {
            input.press_gamepad_button(pad, b);
        }
        if frame == f + 2 {
            input.release_gamepad_button(pad, b);
        }
    }
    // Walk right over the first two coins, then (after saving) left over
    // the third.
    let stick = match frame {
        240..=320 => 1.0,
        380..=475 => -1.0,
        _ => 0.0,
    };
    input.set_gamepad_axis(pad, GamepadAxis::LeftStickX, stick);
}

fn main() -> anyhow::Result<()> {
    box_engine::init_logging();
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/assets");
    let game = Game::new().with_plugin(Demo);
    let mut world = game.new_world();
    world.insert_resource(box_engine::asset_server(&assets)?);
    let mut app = game.app(world);
    if std::env::args().any(|a| a == "--simulate") {
        // Start from no save, so the run is the same every time.
        let _ = std::fs::remove_dir_all(save_dir(&app.world));
        app.add_system(Stage::PostUpdate, simulate);
    }
    box_engine::run(
        app,
        RunOptions {
            width: 1920,
            height: 1080,
            ..RunOptions::new("box-engine game UI")
        },
    )
}

//! In-game UI behaviour, headless (no GPU): focus navigation by gamepad and
//! keyboard, activation and back, input capture, pointer clicks, layout
//! rectangles, string tables and world-anchored labels.

use box_engine::WindowSize;
use box_engine::box_ui::{NodeId, ScaleMode, Size, Strings, Ui, UiEvent, UiEventKind, UiNode};
use box_engine::prelude::*;

const DT: f32 = 1.0 / 60.0;

fn app(width: u32, height: u32) -> App {
    let mut app = box_engine::default_app();
    *app.world.resource_mut::<WindowSize>() = WindowSize {
        width,
        height,
        scale_factor_milli: 1000,
    };
    app
}

fn frame(app: &mut App) {
    app.tick(DT);
    app.world.resource_mut::<Input>().end_frame();
}

/// Taps a gamepad button: one frame down, one frame up.
fn tap(app: &mut App, b: GamepadButton) {
    let pad = GamepadId(0);
    app.world
        .resource_mut::<Input>()
        .press_gamepad_button(pad, b);
    frame(app);
    app.world
        .resource_mut::<Input>()
        .release_gamepad_button(pad, b);
    frame(app);
}

fn key(app: &mut App, k: KeyCode) {
    app.world.resource_mut::<Input>().press_key(k);
    frame(app);
    app.world.resource_mut::<Input>().release_key(k);
    frame(app);
}

fn connect(app: &mut App) {
    app.world
        .resource_mut::<Input>()
        .connect_gamepad(GamepadId(0), "test pad");
}

struct Log {
    cursor: EventCursor<UiEvent>,
}

impl Log {
    fn new() -> Self {
        Self {
            cursor: EventCursor::default(),
        }
    }

    fn take(&mut self, app: &App) -> Vec<UiEvent> {
        self.cursor
            .read(app.world.resource::<Events<UiEvent>>())
            .cloned()
            .collect()
    }
}

fn ui(app: &mut App) -> &mut Ui {
    app.world.resource_mut::<Ui>()
}

fn id_of(app: &App, n: Option<NodeId>) -> Option<String> {
    n.and_then(|n| app.world.resource::<Ui>().id_of(n).map(str::to_string))
}

fn focused(app: &App) -> Option<String> {
    id_of(app, app.world.resource::<Ui>().focused())
}

/// A modal menu: a column of three buttons, then a row of two.
fn menu(app: &mut App) -> NodeId {
    ui(app).add(
        None,
        UiNode::panel()
            .modal()
            .anchored([0.5, 0.5], [0.0, 0.0])
            .children([
                UiNode::button("a", "a"),
                UiNode::button("b", "b"),
                UiNode::button("c", "c"),
                UiNode::row().children([UiNode::button("left", "l"), UiNode::button("right", "r")]),
            ]),
    )
}

#[test]
fn gamepad_moves_focus_spatially_and_wraps() {
    let mut app = app(1920, 1080);
    connect(&mut app);
    menu(&mut app);
    frame(&mut app);
    assert_eq!(focused(&app).as_deref(), Some("a"), "first focusable");
    let mut order = Vec::new();
    for _ in 0..5 {
        tap(&mut app, GamepadButton::DPadDown);
        order.push(focused(&app).unwrap());
    }
    // Down from c lands on the row's nearest overlapping node; from the row
    // it wraps to the top.
    assert_eq!(order, ["b", "c", "left", "a", "b"], "{order:?}");
    tap(&mut app, GamepadButton::DPadUp);
    tap(&mut app, GamepadButton::DPadUp);
    assert_eq!(
        focused(&app).as_deref(),
        Some("left"),
        "up from a wraps to the bottom"
    );
    tap(&mut app, GamepadButton::DPadRight);
    assert_eq!(focused(&app).as_deref(), Some("right"));
    tap(&mut app, GamepadButton::DPadUp);
    assert_eq!(focused(&app).as_deref(), Some("c"));
}

#[test]
fn the_left_stick_navigates_and_repeats_while_held() {
    let mut app = app(1920, 1080);
    connect(&mut app);
    menu(&mut app);
    frame(&mut app);
    let pad = GamepadId(0);
    app.world
        .resource_mut::<Input>()
        .set_gamepad_axis(pad, GamepadAxis::LeftStickY, -1.0);
    frame(&mut app);
    assert_eq!(focused(&app).as_deref(), Some("b"), "first move at once");
    // Held: no repeat before the delay (0.4 s), then every 0.12 s.
    for _ in 0..20 {
        frame(&mut app);
    }
    assert_eq!(focused(&app).as_deref(), Some("b"));
    for _ in 0..8 {
        frame(&mut app);
    }
    assert_eq!(
        focused(&app).as_deref(),
        Some("c"),
        "repeated after the delay"
    );
    app.world
        .resource_mut::<Input>()
        .set_gamepad_axis(pad, GamepadAxis::LeftStickY, 0.0);
    frame(&mut app);
}

#[test]
fn keyboard_arrows_and_tab_follow_document_order() {
    let mut app = app(1920, 1080);
    menu(&mut app);
    frame(&mut app);
    key(&mut app, KeyCode::Down);
    assert_eq!(focused(&app).as_deref(), Some("b"));
    let mut order = Vec::new();
    for _ in 0..4 {
        key(&mut app, KeyCode::Tab);
        order.push(focused(&app).unwrap());
    }
    assert_eq!(order, ["c", "left", "right", "a"]);
}

#[test]
fn confirm_clicks_toggles_and_slides_and_back_reaches_the_modal_root() {
    let mut app = app(1920, 1080);
    connect(&mut app);
    let root = ui(&mut app).add(
        None,
        UiNode::panel().modal().children([
            UiNode::button("go", "go").with_action("start_game"),
            UiNode::toggle("subtitles", "s", false),
            UiNode::slider("volume", "v", 0.5, 0.0, 1.0, 0.25),
        ]),
    );
    let mut log = Log::new();
    frame(&mut app);
    log.take(&app);
    tap(&mut app, GamepadButton::South);
    let e = log.take(&app);
    assert!(e.iter().any(|e| e.is_click("start_game")), "{e:?}");
    assert_eq!(
        e.iter()
            .find(|e| e.kind == UiEventKind::Click)
            .unwrap()
            .id
            .as_deref(),
        Some("go")
    );

    tap(&mut app, GamepadButton::DPadDown);
    tap(&mut app, GamepadButton::South);
    let e = log.take(&app);
    assert!(
        e.iter().any(|e| e.kind == UiEventKind::Toggle(true)),
        "{e:?}"
    );
    let t = ui(&mut app).find(None, "subtitles").unwrap();
    assert_eq!(ui(&mut app).checked(t), Some(true));

    tap(&mut app, GamepadButton::DPadDown);
    log.take(&app);
    // Events last two frames: read after each tap.
    let mut e = Vec::new();
    for _ in 0..2 {
        tap(&mut app, GamepadButton::DPadRight);
        e.extend(log.take(&app));
    }
    let changes: Vec<f32> = e
        .iter()
        .filter_map(|e| match e.kind {
            UiEventKind::Change(v) => Some(v),
            _ => None,
        })
        .collect();
    assert_eq!(changes, [0.75, 1.0], "clamped to the range");
    tap(&mut app, GamepadButton::DPadRight);
    assert!(
        !log.take(&app)
            .iter()
            .any(|e| matches!(e.kind, UiEventKind::Change(_))),
        "no change at the maximum"
    );

    tap(&mut app, GamepadButton::East);
    let e = log.take(&app);
    let back: Vec<_> = e.iter().filter(|e| e.kind == UiEventKind::Back).collect();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].node, root);
}

#[test]
fn a_modal_menu_captures_game_actions_and_the_closing_press_never_reaches_gameplay() {
    #[derive(Default)]
    struct Jumps {
        update: u32,
        fixed: u32,
    }
    let mut app = app(1920, 1080);
    connect(&mut app);
    app.world.resource_mut::<ActionMap>().insert(
        "jump",
        ActionKind::Button {
            bindings: vec![Binding::Gamepad(GamepadButton::South)],
        },
    );
    app.world.insert_resource(Jumps::default());
    app.add_system(Stage::Update, |w| {
        if w.resource::<Actions>().pressed("jump") {
            w.resource_mut::<Jumps>().update += 1;
        }
    });
    app.add_system(Stage::FixedUpdate, |w| {
        if w.resource::<Actions>().pressed("jump") {
            w.resource_mut::<Jumps>().fixed += 1;
        }
    });
    let menu = ui(&mut app).add(
        None,
        UiNode::panel().modal().child(UiNode::button("resume", "r")),
    );
    // The game hides the menu when Resume is clicked.
    let mut cursor = EventCursor::<UiEvent>::default();
    app.add_system(Stage::Update, move |w| {
        let clicked = cursor
            .read(w.resource::<Events<UiEvent>>())
            .any(|e| e.is_click("resume"));
        if clicked {
            w.resource_mut::<Ui>().set_visible(menu, false);
        }
    });
    frame(&mut app);
    assert!(app.world.resource::<Ui>().captures_input());
    tap(&mut app, GamepadButton::South);
    frame(&mut app);
    frame(&mut app);
    assert!(!ui(&mut app).is_visible(menu), "the menu closed");
    assert!(!app.world.resource::<Actions>().is_captured());
    let j = app.world.resource::<Jumps>();
    assert_eq!((j.update, j.fixed), (0, 0), "no jump from the menu press");
    tap(&mut app, GamepadButton::South);
    let j = app.world.resource::<Jumps>();
    assert_eq!((j.update, j.fixed), (1, 1), "gameplay gets input again");
}

/// A modal shown by game code during `Update` (a dialogue box opened by a
/// trigger) captures game input from the next frame's first fixed step: no
/// fixed step sees the stick after the box is up (M8: the hero took one more
/// step into the dialogue).
#[test]
fn a_modal_shown_by_game_code_captures_from_the_next_fixed_step() {
    #[derive(Default)]
    struct Moves(Vec<f32>);
    let mut app = app(1920, 1080);
    connect(&mut app);
    app.world.resource_mut::<ActionMap>().insert(
        "move",
        ActionKind::Axis1 {
            bindings: vec![Axis1Binding::Gamepad {
                axis: GamepadAxis::LeftStickX,
                invert: false,
            }],
            dead_zone: 0.1,
        },
    );
    app.world.insert_resource(Moves::default());
    app.add_system(Stage::FixedUpdate, |w| {
        let v = w.resource::<Actions>().axis1("move");
        w.resource_mut::<Moves>().0.push(v);
    });
    let dialogue = ui(&mut app).add(
        None,
        UiNode::panel()
            .modal()
            .hidden()
            .child(UiNode::button("next", "n")),
    );
    app.world
        .resource_mut::<Input>()
        .set_gamepad_axis(GamepadId(0), GamepadAxis::LeftStickX, 1.0);
    frame(&mut app);
    assert_eq!(app.world.resource::<Moves>().0.last(), Some(&1.0));
    // Game code (an Update system) shows the dialogue.
    app.add_system(Stage::Update, move |w| {
        w.resource_mut::<Ui>().set_visible(dialogue, true);
    });
    frame(&mut app);
    let seen = app.world.resource::<Moves>().0.len();
    frame(&mut app);
    frame(&mut app);
    let after: Vec<f32> = app.world.resource::<Moves>().0[seen..].to_vec();
    assert!(!after.is_empty());
    assert!(
        after.iter().all(|&v| v == 0.0),
        "fixed steps after the modal opened saw {after:?}"
    );
}

#[test]
fn passthrough_actions_reach_the_game_while_captured() {
    let mut app = app(1920, 1080);
    app.world.resource_mut::<ActionMap>().insert(
        "pause",
        ActionKind::Button {
            bindings: vec![Binding::Key(KeyCode::P)],
        },
    );
    ui(&mut app)
        .settings
        .passthrough_actions
        .push("pause".into());
    ui(&mut app).add(
        None,
        UiNode::panel().modal().child(UiNode::button("x", "x")),
    );
    frame(&mut app);
    app.world.resource_mut::<Input>().press_key(KeyCode::P);
    app.tick(DT);
    assert!(app.world.resource::<Actions>().pressed("pause"));
}

#[test]
fn clicking_a_button_sends_click_and_masks_mouse_bound_game_actions() {
    let mut app = app(1920, 1080);
    app.world.resource_mut::<ActionMap>().insert(
        "fire",
        ActionKind::Button {
            bindings: vec![Binding::Mouse(MouseButton::Left)],
        },
    );
    // A non-modal HUD button: game input is not captured, but a click on it
    // is the UI's.
    let b = ui(&mut app).add(
        None,
        UiNode::button("hud_button", "x").anchored([0.0, 0.0], [100.0, 100.0]),
    );
    frame(&mut app);
    let [x, y, w, h] = ui(&mut app).rect(b).unwrap();
    let mut log = Log::new();
    let input = app.world.resource_mut::<Input>();
    input.cursor_position = Some(Vec2::new(x + w / 2.0, y + h / 2.0));
    frame(&mut app);
    assert!(app.world.resource::<Ui>().pointer_over_ui());
    app.world
        .resource_mut::<Input>()
        .press_button(MouseButton::Left);
    app.tick(DT);
    assert!(
        !app.world.resource::<Actions>().pressed("fire"),
        "masked over the UI"
    );
    app.world.resource_mut::<Input>().end_frame();
    app.world
        .resource_mut::<Input>()
        .release_button(MouseButton::Left);
    frame(&mut app);
    assert!(log.take(&app).iter().any(|e| e.is_click("hud_button")));
    // Away from the button the game gets the mouse.
    app.world.resource_mut::<Input>().cursor_position = Some(Vec2::new(1500.0, 900.0));
    frame(&mut app);
    app.world
        .resource_mut::<Input>()
        .press_button(MouseButton::Left);
    app.tick(DT);
    assert!(app.world.resource::<Actions>().pressed("fire"));
}

#[test]
fn layout_places_nodes_by_anchor_padding_gap_fill_and_scale() {
    let mut app = app(1920, 1080);
    let fixed = |w: f32, h: f32| {
        UiNode::spacer()
            .with_width(Size::Px(w))
            .with_height(Size::Px(h))
    };
    let panel = ui(&mut app).add(
        None,
        UiNode::panel()
            .anchored([0.5, 0.5], [0.0, 0.0])
            .with_style(|s| {
                s.padding = Some(box_engine::box_ui::Edges::all(20.0));
                s.gap = Some(10.0);
                s.shadow = None;
            })
            .children([
                fixed(100.0, 50.0).with_id("one"),
                fixed(100.0, 50.0).with_id("two"),
            ]),
    );
    let row = ui(&mut app).add(
        None,
        UiNode::row()
            .anchored([0.0, 1.0], [10.0, -10.0])
            .with_style(|s| {
                s.width = Some(Size::Px(400.0));
                s.gap = Some(0.0);
            })
            .children([
                fixed(100.0, 20.0).with_id("fixed"),
                UiNode::spacer()
                    .with_id("fill1")
                    .with_width(Size::Fill)
                    .with_height(Size::Px(20.0)),
                UiNode::spacer()
                    .with_id("fill3")
                    .with_width(Size::Fill)
                    .with_height(Size::Px(20.0))
                    .with_style(|s| s.grow = Some(3.0)),
            ]),
    );
    frame(&mut app);
    let rect = |app: &mut App, id: &str| {
        let n = ui(app).find(None, id).unwrap();
        ui(app).rect(n).unwrap()
    };
    assert_eq!(ui(&mut app).rect(panel), Some([890.0, 465.0, 140.0, 150.0]));
    assert_eq!(rect(&mut app, "one"), [910.0, 485.0, 100.0, 50.0]);
    assert_eq!(rect(&mut app, "two"), [910.0, 545.0, 100.0, 50.0]);
    assert_eq!(ui(&mut app).rect(row), Some([10.0, 1050.0, 400.0, 20.0]));
    assert_eq!(rect(&mut app, "fixed"), [10.0, 1050.0, 100.0, 20.0]);
    assert_eq!(rect(&mut app, "fill1"), [110.0, 1050.0, 75.0, 20.0]);
    assert_eq!(rect(&mut app, "fill3"), [185.0, 1050.0, 225.0, 20.0]);

    // 1280 x 720: the same layout on the 1920 x 1080 reference, scaled by 2/3.
    *app.world.resource_mut::<WindowSize>() = WindowSize {
        width: 1280,
        height: 720,
        scale_factor_milli: 2000,
    };
    frame(&mut app);
    let r = ui(&mut app).rect(panel).unwrap();
    let expect = [890.0, 465.0, 140.0, 150.0].map(|v: f32| v * 2.0 / 3.0);
    for (a, b) in r.iter().zip(expect) {
        assert!((a - b).abs() < 0.01, "{r:?} vs {expect:?}");
    }
    // DPI mode: points are logical pixels.
    ui(&mut app).set_scale_mode(ScaleMode::Dpi(1.0));
    frame(&mut app);
    let r = ui(&mut app).rect(panel).unwrap();
    assert_eq!(r[2], 280.0, "140 points at scale factor 2");
}

#[test]
fn hidden_nodes_take_no_space_and_text_has_a_size() {
    let mut app = app(1920, 1080);
    let col = ui(&mut app).add(
        None,
        UiNode::column()
            .with_style(|s| s.gap = Some(0.0))
            .children([
                UiNode::text("Hello").with_id("hello"),
                UiNode::text("Hidden").with_id("hidden").hidden(),
            ]),
    );
    frame(&mut app);
    let hello = ui(&mut app).find(None, "hello").unwrap();
    let [_, _, w, h] = ui(&mut app).rect(hello).unwrap();
    assert!(w > 40.0 && h > 20.0, "{w} x {h}");
    let [_, _, _, ch] = ui(&mut app).rect(col).unwrap();
    assert_eq!(ch, h, "the hidden child adds nothing");
    let hidden = ui(&mut app).find(None, "hidden").unwrap();
    assert_eq!(ui(&mut app).rect(hidden), None);
    assert!(!ui(&mut app).is_shown(hidden));
}

#[test]
fn text_comes_from_string_tables_with_fallback_and_placeholders() {
    let mut app = app(1920, 1080);
    {
        let s = app.world.resource_mut::<Strings>();
        s.insert("en", "menu.start", "Start");
        s.insert("en", "hud.coins", "Coins: {count}");
        s.insert("de", "menu.start", "Starten");
    }
    let start = ui(&mut app).add(None, UiNode::label_key("menu.start"));
    let coins = ui(&mut app).add(None, UiNode::label_key("hud.coins").with_arg("count", 3));
    let missing = ui(&mut app).add(None, UiNode::label_key("menu.typo"));
    frame(&mut app);
    assert_eq!(ui(&mut app).text(start).as_deref(), Some("Start"));
    assert_eq!(ui(&mut app).text(coins).as_deref(), Some("Coins: 3"));
    assert_eq!(
        ui(&mut app).text(missing).as_deref(),
        Some("menu.typo"),
        "the key shows"
    );
    app.world.resource_mut::<Strings>().set_language("de");
    ui(&mut app).set_arg(coins, "count", 4);
    frame(&mut app);
    assert_eq!(ui(&mut app).text(start).as_deref(), Some("Starten"));
    assert_eq!(
        ui(&mut app).text(coins).as_deref(),
        Some("Coins: 4"),
        "falls back to en"
    );
}

#[test]
fn world_labels_follow_the_projection_and_hide_behind_the_camera() {
    let mut app = app(1920, 1080);
    app.world.spawn((
        Transform::from_xyz(0.0, 0.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
        Camera::default(),
    ));
    let target = app
        .world
        .spawn((Transform::from_xyz(0.0, 0.0, 0.0), Name::new("Target")));
    let label = ui(&mut app).add(
        None,
        UiNode::text("Here")
            .at_entity("Target", Vec3::ZERO)
            .anchored([0.5, 0.5], [0.0, 0.0]),
    );
    frame(&mut app);
    let [x, y, w, h] = ui(&mut app).rect(label).unwrap();
    assert!(((x + w / 2.0) - 960.0).abs() < 0.5 && ((y + h / 2.0) - 540.0).abs() < 0.5);
    // Moving the entity right moves the label right.
    app.world
        .component_mut::<Transform>(target)
        .unwrap()
        .translation
        .x = 2.0;
    frame(&mut app);
    let [x2, ..] = ui(&mut app).rect(label).unwrap();
    assert!(x2 > x + 100.0, "{x} -> {x2}");
    // Behind the camera: hidden.
    app.world
        .component_mut::<Transform>(target)
        .unwrap()
        .translation
        .z = 20.0;
    frame(&mut app);
    assert!(!ui(&mut app).is_shown(label));
    // Gone: hidden; back (a new entity with the name): shown again.
    app.world.despawn(target);
    frame(&mut app);
    assert!(!ui(&mut app).is_shown(label));
    app.world.spawn((Transform::default(), Name::new("Target")));
    frame(&mut app);
    frame(&mut app);
    assert!(ui(&mut app).is_shown(label));
}

#[test]
fn ui_documents_build_the_same_tree_as_the_builders() {
    let mut app = app(1920, 1080);
    let root = ui(&mut app)
        .add_json(
            None,
            r#"{"kind": "panel", "modal": true, "children": [
                {"kind": "button", "id": "start", "key": "menu.start", "action": "start"},
                {"kind": "slider", "id": "volume", "value": 0.3}
            ]}"#,
        )
        .unwrap();
    assert_eq!(ui(&mut app).children(root).len(), 2);
    let v = ui(&mut app).find(Some(root), "volume").unwrap();
    assert_eq!(ui(&mut app).value(v), Some(0.3));
    assert!(
        ui(&mut app)
            .add_json(None, r#"{"kind": "panel", "colour": 1}"#)
            .is_err()
    );
    assert!(ui(&mut app).remove(root));
    assert!(
        !ui(&mut app).set_text(v, "stale"),
        "stale handles do nothing"
    );
}

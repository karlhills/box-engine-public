//! The script bindings of the game UI (`script` feature, D11): the `ui`
//! and `strings` globals, added to every app by [`UiPlugin`](crate::UiPlugin).
//!
//! Documents are JSON strings or Luau tables with the same fields; nodes
//! are opaque `UiNode` handles (compare with `==`), and every function
//! that takes a node also takes a node `id` string (looked up with
//! `ui.find`). Handlers added with `ui.on(kind, id, fn)` belong to the
//! calling script instance and stop with it; [`forward_ui_events`] turns
//! `UiEvent`s into the `ui` script signal they listen to.
//!
//! ```lua
//! local hud = ui.add({ kind = "text", id = "score", key = "hud.score", args = { score = 0 },
//!                      anchor = { 0, 0 }, offset = { 24, 24 } })
//! ui.set_arg(hud, "score", 10)
//! ui.on("click", "resume", function(ev) ui.set_visible("pause", false) end)
//! ```

use box_core::event::{EventCursor, Events};
use box_core::world::World;
use box_script::convert::{check_entity, from_vector, lua_to_json};
use box_script::mlua::{
    self, Function, Lua, MetaMethod, Result as LuaResult, Table, UserData, UserDataMethods, Value,
    Vector,
};
use box_script::{ScriptApi, ScriptRuntime, ScriptSignals, with_world, with_world_mut};
use glam::Vec3;
use serde_json::{Value as Json, json};

use crate::strings::Strings;
use crate::ui::{NodeId, Ui, UiEvent, UiEventKind};

/// The script signal UI events are sent as.
pub const UI_SIGNAL: &str = "ui";

/// A node handle in Luau.
#[derive(Clone, Copy, Debug, PartialEq)]
struct LuaNode(NodeId);

impl UserData for LuaNode {
    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_meta_method(MetaMethod::Eq, |_, a, b: LuaNode| Ok(a.0 == b.0));
        m.add_meta_method(MetaMethod::ToString, |lua, n, ()| {
            let id = with_world(lua, |w| {
                w.get_resource::<Ui>()
                    .and_then(|ui| ui.id_of(n.0).map(str::to_owned))
            })
            .ok()
            .flatten();
            Ok(match id {
                Some(id) => format!("UiNode(\"{id}\")"),
                None => format!("UiNode({}:{})", n.0.index, n.0.generation),
            })
        });
    }
}

impl mlua::FromLua for LuaNode {
    fn from_lua(v: Value, _: &Lua) -> LuaResult<Self> {
        match v {
            Value::UserData(u) => u.borrow::<LuaNode>().map(|n| *n),
            other => Err(rt(format!("expected a UiNode, got {}", other.type_name()))),
        }
    }
}

fn rt(msg: impl Into<String>) -> mlua::Error {
    mlua::Error::runtime(msg.into())
}

fn ui_ref<R>(lua: &Lua, f: impl FnOnce(&Ui) -> R) -> LuaResult<R> {
    with_world(lua, |w| w.get_resource::<Ui>().map(f))?
        .ok_or_else(|| rt("this world has no game UI (Ui resource)"))
}

fn ui_mut<R>(lua: &Lua, f: impl FnOnce(&mut Ui) -> R) -> LuaResult<R> {
    with_world_mut(lua, |w| w.get_resource_mut::<Ui>().map(f))?
        .ok_or_else(|| rt("this world has no game UI (Ui resource)"))
}

/// A node argument: a `UiNode` or an id string.
fn node(ui: &Ui, v: &Value, what: &str) -> LuaResult<NodeId> {
    match v {
        Value::UserData(u) => u
            .borrow::<LuaNode>()
            .map(|n| n.0)
            .map_err(|_| rt(format!("{what}: expected a UiNode or a node id"))),
        Value::String(s) => {
            let id = s.to_str()?.to_string();
            ui.find(None, &id)
                .ok_or_else(|| rt(format!("{what}: no UI node with id `{id}`")))
        }
        other => Err(rt(format!(
            "{what}: expected a UiNode or a node id, got {}",
            other.type_name()
        ))),
    }
}

/// Runs `f` with the node `v` names (an error when there is none).
fn with_node<R>(
    lua: &Lua,
    v: &Value,
    what: &str,
    f: impl FnOnce(&mut Ui, NodeId) -> R,
) -> LuaResult<R> {
    ui_mut(lua, |ui| node(ui, v, what).map(|n| f(ui, n)))?
}

fn read_node<R>(
    lua: &Lua,
    v: &Value,
    what: &str,
    f: impl FnOnce(&Ui, NodeId) -> R,
) -> LuaResult<R> {
    ui_ref(lua, |ui| node(ui, v, what).map(|n| f(ui, n)))?
}

fn node_value(lua: &Lua, n: Option<NodeId>) -> LuaResult<Value> {
    match n {
        Some(n) => lua.create_userdata(LuaNode(n)).map(Value::UserData),
        None => Ok(Value::Nil),
    }
}

/// `{x, y}` from an array table or a vector.
fn pair(v: &Value, what: &str) -> LuaResult<[f32; 2]> {
    match v {
        Value::Vector(v) => Ok([v.x(), v.y()]),
        Value::Table(t) => Ok([
            t.get::<Option<f32>>(1)?.unwrap_or(0.0),
            t.get::<Option<f32>>(2)?.unwrap_or(0.0),
        ]),
        Value::Nil => Ok([0.0, 0.0]),
        other => Err(rt(format!(
            "{what}: expected {{x, y}} or a vector, got {}",
            other.type_name()
        ))),
    }
}

/// A document from a JSON string or a Luau table. Values of `args` tables
/// may be numbers or booleans (they become text).
fn document(v: &Value) -> LuaResult<String> {
    match v {
        Value::String(s) => Ok(s.to_str()?.to_string()),
        Value::Table(_) => {
            let mut json = lua_to_json(v, &|_| None).map_err(|e| rt(format!("ui.add: {e}")))?;
            stringify_args(&mut json);
            Ok(json.to_string())
        }
        other => Err(rt(format!(
            "ui.add: expected a JSON string or a table, got {}",
            other.type_name()
        ))),
    }
}

fn stringify_args(json: &mut Json) {
    match json {
        Json::Object(map) => {
            for (k, v) in map.iter_mut() {
                if k == "args"
                    && let Json::Object(args) = v
                {
                    for a in args.values_mut() {
                        let text = match &*a {
                            Json::String(_) => continue,
                            Json::Number(n) => match n.as_f64() {
                                Some(f) if f.fract() == 0.0 => format!("{}", f as i64),
                                _ => n.to_string(),
                            },
                            other => other.to_string(),
                        };
                        *a = Json::String(text);
                    }
                } else {
                    stringify_args(v);
                }
            }
        }
        Json::Array(items) => items.iter_mut().for_each(stringify_args),
        _ => {}
    }
}

/// Text for a placeholder value.
fn arg_text(v: &Value) -> LuaResult<String> {
    Ok(match v {
        Value::String(s) => s.to_str()?.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 9.0e15 => format!("{}", *n as i64),
        Value::Number(n) => n.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Nil => String::new(),
        other => {
            return Err(rt(format!(
                "a {} cannot be a text argument",
                other.type_name()
            )));
        }
    })
}

/// `ui`: documents, setters, getters, focus, theme and event handlers.
pub struct UiApi;

macro_rules! setter {
    ($lua:ident, $t:ident, $name:literal, $ty:ty, |$ui:ident, $n:ident, $v:ident| $body:expr) => {
        $t.set(
            $name,
            $lua.create_function(|lua, (target, $v): (Value, $ty)| {
                with_node(lua, &target, concat!("ui.", $name), |$ui, $n| $body)
            })?,
        )?;
    };
}

macro_rules! getter {
    ($lua:ident, $t:ident, $name:literal, |$ui:ident, $n:ident| $body:expr) => {
        $t.set(
            $name,
            $lua.create_function(|lua, target: Value| {
                read_node(lua, &target, concat!("ui.", $name), |$ui, $n| $body)
            })?,
        )?;
    };
}

impl ScriptApi for UiApi {
    fn name(&self) -> &str {
        "ui"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "add",
            lua.create_function(|lua, (doc, parent): (Value, Value)| {
                let json = document(&doc)?;
                let id = ui_mut(lua, |ui| -> LuaResult<NodeId> {
                    let parent = if parent.is_nil() {
                        None
                    } else {
                        Some(node(ui, &parent, "ui.add parent")?)
                    };
                    ui.add_json(parent, &json)
                        .map_err(|e| rt(format!("ui.add: {e}")))
                })??;
                node_value(lua, Some(id))
            })?,
        )?;
        t.set(
            "find",
            lua.create_function(|lua, (id, scope): (String, Value)| {
                let found = ui_ref(lua, |ui| -> LuaResult<Option<NodeId>> {
                    let scope = if scope.is_nil() {
                        None
                    } else {
                        Some(node(ui, &scope, "ui.find scope")?)
                    };
                    Ok(ui.find(scope, &id))
                })??;
                node_value(lua, found)
            })?,
        )?;
        t.set(
            "remove",
            lua.create_function(|lua, target: Value| {
                ui_mut(lua, |ui| match node(ui, &target, "ui.remove") {
                    Ok(n) => ui.remove(n),
                    Err(_) => false,
                })
            })?,
        )?;
        t.set(
            "contains",
            lua.create_function(|lua, target: Value| {
                ui_ref(lua, |ui| {
                    node(ui, &target, "ui.contains").is_ok_and(|n| ui.contains(n))
                })
            })?,
        )?;
        setter!(lua, t, "set_text", String, |ui, n, v| ui.set_text(n, v));
        setter!(lua, t, "set_key", String, |ui, n, v| ui.set_key(n, v));
        t.set(
            "set_arg",
            lua.create_function(|lua, (target, name, value): (Value, String, Value)| {
                let text = arg_text(&value)?;
                with_node(lua, &target, "ui.set_arg", |ui, n| {
                    ui.set_arg(n, &name, text)
                })
            })?,
        )?;
        t.set(
            "set_args",
            lua.create_function(|lua, (target, args): (Value, Table)| {
                let mut list = Vec::new();
                for pair in args.pairs::<String, Value>() {
                    let (k, v) = pair?;
                    list.push((k, arg_text(&v)?));
                }
                with_node(lua, &target, "ui.set_args", |ui, n| {
                    list.into_iter()
                        .fold(true, |ok, (k, v)| ui.set_arg(n, &k, v) && ok)
                })
            })?,
        )?;
        setter!(lua, t, "set_visible", bool, |ui, n, v| ui.set_visible(n, v));
        setter!(lua, t, "set_enabled", bool, |ui, n, v| ui.set_enabled(n, v));
        setter!(lua, t, "set_value", f32, |ui, n, v| ui.set_value(n, v));
        setter!(lua, t, "set_checked", bool, |ui, n, v| ui.set_checked(n, v));
        setter!(lua, t, "set_class", String, |ui, n, v| ui.set_class(n, &v));
        setter!(lua, t, "set_image", String, |ui, n, v| ui.set_image(n, v));
        setter!(lua, t, "set_action", Option<String>, |ui, n, v| ui
            .set_action(n, v));
        t.set(
            "set_range",
            lua.create_function(|lua, (target, min, max): (Value, f32, f32)| {
                with_node(lua, &target, "ui.set_range", |ui, n| {
                    ui.set_range(n, min, max)
                })
            })?,
        )?;
        t.set(
            "set_style",
            lua.create_function(|lua, (target, style): (Value, Value)| {
                let json = match &style {
                    Value::String(s) => serde_json::from_str::<Json>(&s.to_str()?)
                        .map_err(|e| rt(format!("ui.set_style: {e}")))?,
                    other => lua_to_json(other, &|_| None)
                        .map_err(|e| rt(format!("ui.set_style: {e}")))?,
                };
                let style: crate::style::Style =
                    serde_json::from_value(json).map_err(|e| rt(format!("ui.set_style: {e}")))?;
                with_node(lua, &target, "ui.set_style", |ui, n| ui.set_style(n, style))
            })?,
        )?;
        t.set(
            "set_anchor",
            lua.create_function(|lua, (target, anchor, offset): (Value, Value, Value)| {
                let (a, o) = (
                    pair(&anchor, "ui.set_anchor")?,
                    pair(&offset, "ui.set_anchor")?,
                );
                with_node(lua, &target, "ui.set_anchor", |ui, n| {
                    ui.set_anchor(n, a, o)
                })
            })?,
        )?;
        t.set(
            "anchor_to_entity",
            lua.create_function(
                |lua, (target, entity, offset): (Value, Value, Option<Vector>)| {
                    let e = check_entity(&entity, "ui.anchor_to_entity")?;
                    let offset = offset.map_or(Vec3::ZERO, from_vector);
                    with_node(lua, &target, "ui.anchor_to_entity", |ui, n| {
                        ui.anchor_to_entity(n, e, offset)
                    })
                },
            )?,
        )?;
        setter!(lua, t, "anchor_to_point", Vector, |ui, n, v| ui
            .anchor_to_point(n, from_vector(v)));
        t.set(
            "clear_world_anchor",
            lua.create_function(|lua, target: Value| {
                with_node(lua, &target, "ui.clear_world_anchor", |ui, n| {
                    ui.clear_world_anchor(n)
                })
            })?,
        )?;
        t.set(
            "focus",
            lua.create_function(|lua, target: Value| {
                with_node(lua, &target, "ui.focus", |ui, n| ui.focus(n))
            })?,
        )?;
        t.set(
            "clear_focus",
            lua.create_function(|lua, ()| ui_mut(lua, Ui::clear_focus))?,
        )?;
        t.set(
            "focused",
            lua.create_function(|lua, ()| {
                let n = ui_ref(lua, Ui::focused)?;
                node_value(lua, n)
            })?,
        )?;
        t.set(
            "active_modal",
            lua.create_function(|lua, ()| {
                let n = ui_ref(lua, Ui::active_modal)?;
                node_value(lua, n)
            })?,
        )?;
        t.set(
            "captures_input",
            lua.create_function(|lua, ()| ui_ref(lua, Ui::captures_input))?,
        )?;
        getter!(lua, t, "text", |ui, n| ui.text(n));
        getter!(lua, t, "value", |ui, n| ui.value(n));
        getter!(lua, t, "is_checked", |ui, n| ui.checked(n));
        box_script::deprecated_alias(lua, &t, "ui", "checked", "is_checked")?;
        getter!(lua, t, "is_visible", |ui, n| ui.is_visible(n));
        getter!(lua, t, "is_shown", |ui, n| ui.is_shown(n));
        getter!(lua, t, "is_enabled", |ui, n| ui.is_enabled(n));
        getter!(lua, t, "kind", |ui, n| ui.kind(n).map(|k| k.class()));
        getter!(lua, t, "id", |ui, n| ui.id_of(n).map(str::to_owned));
        t.set(
            "rect",
            lua.create_function(|lua, target: Value| {
                let r = read_node(lua, &target, "ui.rect", |ui, n| ui.rect(n))?;
                r.map(|[x, y, w, h]| {
                    let t = lua.create_table_with_capacity(0, 4)?;
                    t.raw_set("x", x)?;
                    t.raw_set("y", y)?;
                    t.raw_set("w", w)?;
                    t.raw_set("h", h)?;
                    Ok::<_, mlua::Error>(t)
                })
                .transpose()
            })?,
        )?;
        t.set(
            "parent",
            lua.create_function(|lua, target: Value| {
                let p = read_node(lua, &target, "ui.parent", |ui, n| ui.parent(n))?;
                node_value(lua, p)
            })?,
        )?;
        t.set(
            "children",
            lua.create_function(|lua, target: Value| {
                let kids = read_node(lua, &target, "ui.children", |ui, n| ui.children(n).to_vec())?;
                let out = lua.create_table_with_capacity(kids.len(), 0)?;
                for (i, k) in kids.into_iter().enumerate() {
                    out.raw_set(i + 1, node_value(lua, Some(k))?)?;
                }
                Ok(out)
            })?,
        )?;
        t.set(
            "set_theme",
            lua.create_function(|lua, path: String| ui_mut(lua, |ui| ui.set_theme(path)))?,
        )?;
        t.set(
            "set_passthrough_actions",
            lua.create_function(|lua, actions: Vec<String>| {
                ui_mut(lua, |ui| ui.settings.passthrough_actions = actions)
            })?,
        )?;
        t.set(
            "on",
            lua.create_function(
                |lua, (kind, id, handler): (String, Value, Option<Function>)| {
                    // `ui.on(kind, handler)` or `ui.on(kind, id, handler)`.
                    let (id, handler) = match (id, handler) {
                        (Value::Function(f), None) => (None, f),
                        (Value::Nil, Some(f)) => (None, f),
                        (Value::String(s), Some(f)) => (Some(s.to_str()?.to_string()), f),
                        _ => {
                            return Err(rt(
                                "ui.on: expected (kind, handler) or (kind, id, handler)",
                            ));
                        }
                    };
                    const KINDS: [&str; 7] =
                        ["click", "change", "toggle", "focus", "blur", "back", "any"];
                    if !KINDS.contains(&kind.as_str()) {
                        return Err(rt(format!(
                            "ui.on: unknown event kind `{kind}` (one of {})",
                            KINDS.join(", ")
                        )));
                    }
                    let filter = lua.create_function(move |lua, ev: Table| {
                        let k: String = ev.get("kind")?;
                        if kind != "any" && k != kind {
                            return Ok(());
                        }
                        if let Some(want) = &id
                            && ev.get::<Option<String>>("id")?.as_deref() != Some(want.as_str())
                        {
                            return Ok(());
                        }
                        // The node arrives as [index, generation].
                        if let Value::Table(n) = ev.get::<Value>("node")? {
                            let node = NodeId {
                                index: n.get(1)?,
                                generation: n.get(2)?,
                            };
                            ev.set("node", node_value(lua, Some(node))?)?;
                        }
                        handler.call::<()>(ev)
                    })?;
                    box_script::subscribe(lua, UI_SIGNAL, filter)
                },
            )?,
        )?;
        t.set(
            "off",
            lua.create_function(|lua, id: u64| box_script::unsubscribe(lua, id))?,
        )?;
        g.set("ui", t)
    }

    fn definitions(&self) -> String {
        r#"--- A handle to a node of the game UI (returned by `ui.add`, `ui.find`, `ui.children`, events). Compares with `==`. A handle to a removed node makes functions fail; `ui.contains` checks.
declare extern type UiNode with
end

--- A node argument: a `UiNode`, or a node `id` string (looked up like `ui.find(id)`, failing when there is none).
type UiTarget = UiNode | string
--- What happened to a node: a button clicked, a slider changed, a toggle toggled, focus gained or lost, or back pressed (Escape, Backspace, gamepad East) in a modal (the event is on the modal root).
type UiEventKind = "click" | "change" | "toggle" | "focus" | "blur" | "back"
--- A UI event, as `ui.on` handlers receive it.
type UiEvent = {
    --- The node the event happened on.
    node: UiNode,
    --- The node's `id`, if it has one.
    id: string?,
    --- The node's `action`, if it has one.
    action: string?,
    --- What happened.
    kind: UiEventKind,
    --- The new slider value (`change` events).
    value: number?,
    --- The new toggle state (`toggle` events).
    checked: boolean?,
}

--- The game UI: menus, HUDs and labels over the game, built from documents (JSON text or tables) and changed through setters. Every setter returns true if it changed something.
declare ui: {
    --- Adds a document (a node with its children) under `parent`, or as a new root, and returns its node. A document is a JSON string or a table with the same fields: `kind`, `id`, `text` or `key`, `args`, `class`, `style`, `children`, `anchor`, `modal` and so on (see the overview).
    add: (document: string | { [string]: any }, parent: UiTarget?) -> UiNode,
    --- The first node with this `id` (depth first) under `scope` (inclusive), or in the whole UI; nil when there is none.
    find: (id: string, scope: UiTarget?) -> UiNode?,
    --- Removes a node and its descendants. Returns false if it was not there.
    remove: (node: UiTarget) -> boolean,
    --- True if the node still exists.
    contains: (node: UiTarget) -> boolean,
    --- Shows literal text (and clears a string table key).
    set_text: (node: UiTarget, text: string) -> boolean,
    --- Shows the text of a string table key (in the current language; it follows language switches).
    set_key: (node: UiTarget, key: string) -> boolean,
    --- Sets the value of a `{name}` placeholder in the node's text.
    set_arg: (node: UiTarget, name: string, value: string | number | boolean) -> boolean,
    --- Sets several placeholder values at once.
    set_args: (node: UiTarget, args: { [string]: string | number | boolean }) -> boolean,
    --- Shows or hides the node (and its children).
    set_visible: (node: UiTarget, visible: boolean) -> boolean,
    --- Enables or disables an interactive node (a disabled button cannot be focused or clicked).
    set_enabled: (node: UiTarget, enabled: boolean) -> boolean,
    --- Sets a slider or progress value (clamped to its range). Sends no event.
    set_value: (node: UiTarget, value: number) -> boolean,
    --- Sets a slider or progress range.
    set_range: (node: UiTarget, min: number, max: number) -> boolean,
    --- Sets a toggle. Sends no event.
    set_checked: (node: UiTarget, checked: boolean) -> boolean,
    --- Replaces the node's extra theme classes (space separated).
    set_class: (node: UiTarget, class: string) -> boolean,
    --- Replaces the node's inline style (a JSON string or a table: `background`, `text_color`, `text_size`, `padding`, `gap`, `radius`, `width`, `height` and the other theme style fields).
    set_style: (node: UiTarget, style: string | { [string]: any }) -> boolean,
    --- Shows another image asset (assets-relative path) in an image node.
    set_image: (node: UiTarget, path: string) -> boolean,
    --- Sets (or clears, with nil) the action name sent with the node's events.
    set_action: (node: UiTarget, action: string?) -> boolean,
    --- Places a root node on the screen: `anchor` from {0, 0} (top left) to {1, 1} (bottom right), also its pivot; `offset` in reference points.
    set_anchor: (node: UiTarget, anchor: { number } | vector, offset: ({ number } | vector)?) -> boolean,
    --- Makes a root node follow an entity's position plus `offset` (metres), such as a name tag or a health bar. It is hidden while the point is off screen.
    anchor_to_entity: (node: UiTarget, entity: Entity, offset: vector?) -> boolean,
    --- Makes a root node follow a world point.
    anchor_to_point: (node: UiTarget, point: vector) -> boolean,
    --- Puts a world-anchored root node back at its screen anchor.
    clear_world_anchor: (node: UiTarget) -> boolean,
    --- Moves keyboard and gamepad focus to a focusable node (sends `blur` and `focus` events).
    focus: (node: UiTarget) -> boolean,
    --- Removes focus from every node.
    clear_focus: () -> (),
    --- The focused node, or nil.
    focused: () -> UiNode?,
    --- The visible modal root that owns focus and captures game input, or nil.
    active_modal: () -> UiNode?,
    --- True while a modal root captures game input (gameplay actions read as idle).
    captures_input: () -> boolean,
    --- The node's text as last laid out (string table and placeholders applied, inline tags removed), or nil.
    text: (node: UiTarget) -> string?,
    --- A slider or progress value, or nil for other kinds.
    value: (node: UiTarget) -> number?,
    --- A toggle's state, or nil for other kinds.
    is_checked: (node: UiTarget) -> boolean?,
    --- Deprecated: use `is_checked`.
    checked: (node: UiTarget) -> boolean?,
    --- True unless the node itself is hidden (`set_visible(false)`).
    is_visible: (node: UiTarget) -> boolean,
    --- True if the node was drawn in the last layout (it and every ancestor visible, a world anchor on screen).
    is_shown: (node: UiTarget) -> boolean,
    --- True unless the node is disabled.
    is_enabled: (node: UiTarget) -> boolean,
    --- The node's kind ("panel", "column", "row", "stack", "text", "image", "button", "slider", "toggle", "list", "progress" or "spacer").
    kind: (node: UiTarget) -> string?,
    --- The node's `id`, or nil.
    id: (node: UiTarget) -> string?,
    --- The node's rectangle from the last layout in physical pixels (x, y from the top left; w, h), or nil before its first layout.
    rect: (node: UiTarget) -> { x: number, y: number, w: number, h: number }?,
    --- The parent node, or nil for a root.
    parent: (node: UiTarget) -> UiNode?,
    --- The child nodes, in order.
    children: (node: UiTarget) -> { UiNode },
    --- Uses a theme file (`*.theme.json`, assets-relative), merged over the built-in theme and hot-reloaded.
    set_theme: (path: string) -> (),
    --- Actions that still reach the game while a modal captures input (a "pause" action that also closes the pause menu).
    set_passthrough_actions: (actions: { string }) -> (),
    --- Calls `handler(event)` for UI events of `kind` ("any" for all kinds), only for the node with `id` when given (`ui.on(kind, handler)` or `ui.on(kind, id, handler)`). Handlers run in the scripts' Update stage, belong to the calling script instance and stop with it. Returns an id for `ui.off`.
    on: ((kind: UiEventKind | "any", handler: (event: UiEvent) -> ()) -> number) & ((kind: UiEventKind | "any", id: string?, handler: (event: UiEvent) -> ()) -> number),
    --- Removes a handler added with `ui.on`. Returns true if it existed.
    off: (handler: number) -> boolean,
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        r#"The game UI is retained: build nodes once with `ui.add`, then change them with setters when something happens (not every frame), and react to clicks with `ui.on`. Nodes are laid out by the engine and styled by a theme (`ui.set_theme`). Any function that takes a node also takes its `id` string.

A document is a node table (or the same as JSON text): `kind` (`panel`, `column`, `row`, `stack`, `text`, `image`, `button`, `slider`, `toggle`, `list`, `progress`, `spacer`), `id`, `class`, `text` or `key` (a string table key) with `args` for `{name}` placeholders, `image`, `style`, `hidden`, `disabled`, `value`/`min`/`max`/`step` (sliders, progress), `checked` (toggles), `action` (sent with events), and on roots `anchor`, `offset`, `layer` and `modal` (a modal root owns focus and keeps gameplay input from the game while visible). Children go in `children`. Text supports inline tags such as `[color=$accent]...[/color]`.

```lua
function Hud.on_start(self)
    ui.add({
        kind = "panel", id = "hud", anchor = { 0, 0 }, offset = { 24, 24 },
        children = {
            { kind = "text", id = "score", key = "hud.score", args = { score = 0 } },
        },
    })
    ui.set_arg("score", "score", 10)

    local menu = ui.add({
        kind = "panel", id = "pause", modal = true, hidden = true, anchor = { 0.5, 0.5 },
        children = {
            { kind = "button", id = "resume", text = "Resume" },
            { kind = "button", id = "quit", text = "Quit" },
        },
    })
    ui.on("click", "resume", function()
        ui.set_visible(menu, false)
        time.set_paused(false)
    end)
end
```"#
    }
}

/// `strings`: text from the string tables.
pub struct StringsApi;

fn strings<R>(lua: &Lua, f: impl FnOnce(&Strings) -> R) -> LuaResult<R> {
    with_world(lua, |w| w.get_resource::<Strings>().map(f))?
        .ok_or_else(|| rt("this world has no string tables (Strings resource)"))
}

impl ScriptApi for StringsApi {
    fn name(&self) -> &str {
        "strings"
    }

    fn install(&self, lua: &Lua, g: &Table) -> LuaResult<()> {
        let t = lua.create_table()?;
        t.set(
            "get",
            lua.create_function(|lua, key: String| strings(lua, |s| s.get(&key)))?,
        )?;
        t.set(
            "has",
            lua.create_function(|lua, key: String| strings(lua, |s| s.has(&key)))?,
        )?;
        t.set(
            "format",
            lua.create_function(|lua, (key, args): (String, Option<Table>)| {
                let mut list = Vec::new();
                if let Some(args) = args {
                    for pair in args.pairs::<String, Value>() {
                        let (k, v) = pair?;
                        list.push((k, arg_text(&v)?));
                    }
                }
                let refs: Vec<(&str, &str)> =
                    list.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                strings(lua, |s| s.format(&key, &refs))
            })?,
        )?;
        t.set(
            "language",
            lua.create_function(|lua, ()| strings(lua, |s| s.language().to_string()))?,
        )?;
        t.set(
            "set_language",
            lua.create_function(|lua, language: String| {
                with_world_mut(lua, |w| {
                    w.resource_or_default::<Strings>().set_language(&language)
                })
            })?,
        )?;
        g.set("strings", t)
    }

    fn definitions(&self) -> String {
        r#"--- Localized text from the project's string tables.
declare strings: {
    --- The text for `key` (dotted, such as "menu.resume") in the active language, else the fallback language, else the key itself (a missing key is warned about once).
    get: (key: string) -> string,
    --- True if `key` has text in the active or fallback language.
    has: (key: string) -> boolean,
    --- The text for `key` with `{name}` placeholders filled from `args`.
    format: (key: string, args: { [string]: string | number | boolean }?) -> string,
    --- The active language code, such as "en".
    language: () -> string,
    --- Switches the language; its tables load on the next frame and UI text shown with `key` updates.
    set_language: (language: string) -> (),
}
"#
        .into()
    }

    fn doc(&self) -> &str {
        r#"String tables are JSON files under `assets/` named by language: `strings/en.strings.json`, `strings/de.strings.json` (or `strings/dialogue.de.strings.json`; files of one language are merged). Nested objects make dotted keys, and `{name}` placeholders are filled from arguments. UI nodes show table text with `key` (`ui.set_key`), and follow language switches.

```json
{ "hud": { "score": "Score {score}" }, "menu": { "resume": "Resume" } }
```

```lua
function Lang.on_start(self)
    log.info(strings.format("hud.score", { score = 42 }))   -- "Score 42"
    strings.set_language("de")
end
```"#
    }
}

/// Update system (after `ui_input`): sends each `UiEvent` to scripts as
/// the `ui` signal (`{ node, id, action, kind, value, checked }`) while
/// the world runs scripts.
pub fn forward_ui_events(cursor: &mut EventCursor<UiEvent>, world: &mut World) {
    let Some(events) = world.get_resource::<Events<UiEvent>>() else {
        return;
    };
    let list: Vec<UiEvent> = cursor.read(events).cloned().collect();
    if list.is_empty() || !world.contains_resource::<ScriptRuntime>() {
        return;
    }
    let signals = world.resource_or_default::<ScriptSignals>();
    for ev in list {
        let mut payload = json!({
            "node": [ev.node.index, ev.node.generation],
            "id": ev.id,
            "action": ev.action,
            "kind": ev.kind.name(),
        });
        match ev.kind {
            UiEventKind::Change(v) => payload["value"] = json!(v),
            UiEventKind::Toggle(c) => payload["checked"] = json!(c),
            _ => {}
        }
        signals.send(UI_SIGNAL, payload);
    }
}

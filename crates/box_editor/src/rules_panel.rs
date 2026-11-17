//! The rule list (View ▸ Rules, palette "Rules"; docs/PROCEDURAL_DIRECTION.md
//! §6): every rule of the world shown (the edit world, or the play world
//! during Play) with its intent types, intents, parts, last run time,
//! warnings and an enable switch, in a floating window over the editor.
//!
//! **Disabling a rule** is a per-session debugging switch of this editor:
//! the rule stops running in the edit world and in Play started from this
//! editor, and its parts and warnings are removed (reconciled to nothing);
//! enabling it again runs it at once, so its parts come back with the same
//! uids. Nothing is saved: scenes, the player and `box` always run every
//! rule, so a rule cannot stay off by accident.
//!
//! Cost: the switches are written into the world's `RuleRegistry` once per
//! frame (one flag per rule); the window, while open, lists the registry's
//! rules (a handful of rows).

use std::collections::BTreeSet;

use box_engine::box_proc::rule::{RuleInfo, RuleRegistry};
use egui::{Align2, Context, RichText};

use crate::path_tool::{Closing, PathTool, PathToolConfig};
use crate::state::EditorState;
use crate::theme::Tokens;

/// The panel's state: open, and the rules switched off this session.
#[derive(Debug, Default)]
pub(crate) struct RulesPanel {
    pub open: bool,
    pub disabled: BTreeSet<String>,
    /// The Draw Path tool for script rules is registered.
    pub path_tool: bool,
}

impl EditorState {
    /// Whether the rule list window is open.
    pub fn rules_panel_open(&self) -> bool {
        self.rules_panel.open
    }

    /// Opens or closes the rule list window.
    pub fn set_rules_panel(&mut self, open: bool) {
        self.rules_panel.open = open;
    }

    /// The rules of the shown world (the play world during Play), as the
    /// rule list shows them.
    pub fn rule_rows(&self) -> Vec<RuleInfo> {
        self.world
            .get_resource::<RuleRegistry>()
            .map(|r| r.rules().cloned().collect())
            .unwrap_or_default()
    }

    /// Whether rule `name` runs in this editor session.
    pub fn rule_enabled(&self, name: &str) -> bool {
        !self.rules_panel.disabled.contains(name)
    }

    /// Switches rule `name` on or off for this session (see the module
    /// docs): applied to the shown world at once, and to every world the
    /// editor runs rules in afterwards.
    pub fn set_rule_enabled(&mut self, name: &str, enabled: bool) {
        if enabled {
            self.rules_panel.disabled.remove(name);
        } else {
            self.rules_panel.disabled.insert(name.to_owned());
        }
        self.apply_rule_switches();
        tracing::info!(
            "rule `{name}` {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    /// Writes the session's switches into the shown world's rules (once per
    /// frame, before rules run; one flag per rule). A switched-off script
    /// rule the world has not found yet starts switched off.
    pub(crate) fn apply_rule_switches(&mut self) {
        let disabled = &self.rules_panel.disabled;
        let Some(registry) = self.world.get_resource_mut::<RuleRegistry>() else {
            return;
        };
        let mut wrong: Vec<(&'static str, bool)> = Vec::new();
        let mut script_rules = false;
        for r in registry.rules() {
            let want = !disabled.contains(r.name);
            if r.enabled != want {
                wrong.push((r.name, want));
            }
            script_rules |= r.intent == box_engine::box_script::rules::INTENT_TYPE;
        }
        for (name, want) in wrong {
            registry.set_enabled(name, want);
        }
        for name in disabled {
            if registry.get(name).is_none() {
                registry.set_enabled(name, false);
            }
        }
        if script_rules && !self.rules_panel.path_tool {
            self.rules_panel.path_tool = true;
            self.register_tool(PathTool::new(script_path_tool()));
        }
    }
}

/// The Draw Path tool's id (palette action `tool.draw_path`), registered
/// once the shown world has a script rule.
pub const SCRIPT_PATH_TOOL: &str = "draw_path";

/// Draw Path: the points of `ScriptIntent`s (script rule paths: fences,
/// rows, outlines). New paths copy the selected one's rule and settings.
pub fn script_path_tool() -> PathToolConfig {
    PathToolConfig::new(SCRIPT_PATH_TOOL, "Path", "ScriptIntent", "points")
        .name("Draw Path")
        .tooltip(
            "Draw script rule paths (fences, rows, outlines): select one first, new paths copy \
             its rule and settings. Click to place points (they snap to other paths), Enter or \
             click the last point to finish, click the first point to close, Backspace removes \
             the last point, Esc cancels. Drag a selected path's points to move them.",
        )
        .entity_name("Path")
        .closing(Closing::RepeatFirstPoint)
        .copy_selected()
}

fn ms(d: Option<std::time::Duration>) -> String {
    match d {
        Some(d) => format!("{:.2} ms", d.as_secs_f64() * 1000.0),
        None => "—".into(),
    }
}

/// The rule list window, while open.
pub(crate) fn rules_window(ctx: &Context, state: &mut EditorState, t: &Tokens) {
    if !state.rules_panel.open {
        return;
    }
    let rows = state.rule_rows();
    let mut open = true;
    let mut toggles: Vec<(&'static str, bool)> = Vec::new();
    egui::Window::new("Rules")
        .id(egui::Id::unique("box_editor_rules_panel"))
        .open(&mut open)
        .collapsible(false)
        .fade_in(false)
        .fade_out(false)
        .resizable(true)
        .default_width(560.0)
        .anchor(Align2::RIGHT_TOP, egui::vec2(-340.0, 130.0))
        .show(ctx, |ui| {
            if rows.is_empty() {
                ui.label(
                    RichText::new(
                        "No rules. A game adds them with app.add_rule in its plugin; \
                         they turn saved intent (a wall path) into generated parts.",
                    )
                    .color(t.text_weak),
                );
                return;
            }
            let playing = state.is_playing();
            ui.label(
                RichText::new(if playing {
                    "Play world. Switching a rule off removes its parts until it is on again \
                     (this session only; scenes and the player run every rule)."
                } else {
                    "Switching a rule off removes its parts until it is on again \
                     (this session only; scenes and the player run every rule)."
                })
                .color(t.text_weak)
                .small(),
            );
            ui.add_space(6.0);
            egui::Grid::new("box_editor_rules_grid")
                .striped(true)
                .num_columns(7)
                .spacing(egui::vec2(14.0, 6.0))
                .show(ui, |ui| {
                    for h in [
                        "On", "Rule", "Intent", "Intents", "Parts", "Last run", "Warnings",
                    ] {
                        ui.label(RichText::new(h).color(t.text_weak).small());
                    }
                    ui.end_row();
                    for r in &rows {
                        let mut on = state.rule_enabled(r.name);
                        if crate::widgets::checkbox_labeled(ui, &mut on, "")
                            .on_hover_text(if on {
                                "Switch off: stop the rule and remove its parts (this session)"
                            } else {
                                "Switch on: run the rule again"
                            })
                            .changed()
                        {
                            toggles.push((r.name, on));
                        }
                        let name = RichText::new(r.name).font(t.mono());
                        ui.label(if r.enabled {
                            name.color(t.text_strong)
                        } else {
                            name.color(t.text_faint)
                        });
                        let intent = if r.reads.is_empty() {
                            r.intent.to_owned()
                        } else {
                            format!("{} (reads {})", r.intent, r.reads.join(", "))
                        };
                        ui.label(RichText::new(intent).color(t.text));
                        ui.label(RichText::new(r.intents.to_string()).color(t.text));
                        ui.label(RichText::new(r.parts.to_string()).color(t.text));
                        ui.label(RichText::new(ms(r.last_run)).color(t.text))
                            .on_hover_text(format!(
                                "Runs: {}  ·  rule function {}  ·  last run: {} spawned, {} \
                                 updated, {} unchanged, {} removed, {} hidden by overrides",
                                r.runs,
                                ms(r.last_rule),
                                r.last_report.spawned,
                                r.last_report.updated,
                                r.last_report.unchanged,
                                r.last_report.removed,
                                r.last_report.hidden
                            ));
                        let w = RichText::new(r.warnings.to_string());
                        ui.label(if r.warnings > 0 {
                            w.color(t.warn)
                        } else {
                            w.color(t.text)
                        });
                        ui.end_row();
                    }
                });
        });
    for (name, on) in toggles {
        state.set_rule_enabled(name, on);
    }
    if !open {
        state.rules_panel.open = false;
    }
}

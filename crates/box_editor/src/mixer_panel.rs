//! The Mixer (View ▸ Mixer, palette "Mixer"): the game's sound while it
//! plays, in a floating window over the editor.
//!
//! - **Bus faders** are the project's `AudioSettings` (master and one per
//!   bus): moving one applies at once to the play world and is written to
//!   `assets/settings/AudioSettings.json` like a Project Settings edit, so
//!   it stays after Stop. Each bus shows the combined level of its voices.
//! - **Source rows** (during Play): every playing voice, loudest first,
//!   with its bus, distance to the listener, distance attenuation, pan, a
//!   level meter at the listener (after distance, panning, bus and master
//!   volume), solo and mute, and for `AudioSource`s a volume slider (an
//!   ordinary edit of `AudioSource.volume` in the play world; **Keep**
//!   marks it "Keep after Stop" so the tuned value is applied to the scene
//!   at Stop).
//! - **Solo and mute** apply to a source (with the sources below it in the
//!   hierarchy), a one-shot's clip, a bus, or the selected entities (a
//!   group such as "the orchard"). They live in the play world's `Audio`
//!   resource: runtime only, never saved, gone at Stop.
//!
//! Cost while open: one pass over the playing voices (tens) per frame;
//! nothing while closed.

use std::collections::HashMap;

use box_core::identity::Uid;
use box_engine::audio_mixer;
use box_engine::box_audio::{
    Audio, AudioBus, AudioSettings, MixTarget, OutputKind, VoiceInfo, amplitude_db,
};
use egui::{Align2, Color32, Context, CornerRadius, Rect, RichText, Sense, Ui, pos2, vec2};

use crate::state::EditorState;
use crate::theme::Tokens;

/// Meter scale: -60 dBFS .. 0 dBFS.
const METER_FLOOR_DB: f32 = -60.0;
/// Fall of a meter's shown level per second (display smoothing only).
const METER_FALL_DB_PER_S: f32 = 30.0;

/// The window's state.
#[derive(Debug, Default)]
pub(crate) struct MixerPanel {
    pub open: bool,
    /// Shown meter levels (dBFS) by key, falling smoothly.
    shown: HashMap<String, f32>,
    /// A source volume slider drag holds an edit open.
    dragging_volume: bool,
}

impl EditorState {
    /// Whether the Mixer window is open.
    pub fn mixer_open(&self) -> bool {
        self.mixer.open
    }

    /// Opens or closes the Mixer window.
    pub fn set_mixer(&mut self, open: bool) {
        self.mixer.open = open;
    }

    /// Solos (`on`) or unsolos `target` in the playing world. Runtime
    /// only; nothing happens outside Play.
    pub fn audio_solo(&mut self, target: MixTarget, on: bool) -> bool {
        self.play_audio_mut().is_some_and(|a| a.solo(target, on))
    }

    /// Mutes (`on`) or unmutes `target` in the playing world. Runtime only.
    pub fn audio_mute(&mut self, target: MixTarget, on: bool) -> bool {
        self.play_audio_mut().is_some_and(|a| a.mute(target, on))
    }

    /// Clears every solo and mute of the playing world.
    pub fn audio_clear_mix(&mut self) {
        if let Some(a) = self.play_audio_mut() {
            a.clear_mix();
        }
    }

    fn play_audio_mut(&mut self) -> Option<&mut Audio> {
        if !self.is_playing() {
            return None;
        }
        self.world.get_resource_mut::<Audio>()
    }
}

fn db_text(amplitude: f32) -> String {
    let d = amplitude_db(amplitude);
    if d.is_finite() && d > METER_FLOOR_DB {
        format!("{d:.0} dB")
    } else {
        "—".into()
    }
}

/// Meter colour by level: green, yellow above -12 dB, red above -3 dB.
pub(crate) fn level_color(db: f32) -> Color32 {
    if db > -3.0 {
        Color32::from_rgb(232, 84, 72)
    } else if db > -12.0 {
        Color32::from_rgb(226, 190, 72)
    } else {
        Color32::from_rgb(92, 196, 120)
    }
}

/// A horizontal level meter: `db` filled, a tick at `peak_db`; grey when
/// the sound is not heard (muted or soloed out).
fn meter(ui: &mut Ui, t: &Tokens, width: f32, db: f32, peak_db: f32, heard: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, t.row_h * 0.45), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(2), t.field);
    let f = |d: f32| ((d - METER_FLOOR_DB) / -METER_FLOOR_DB).clamp(0.0, 1.0);
    let fill = Rect::from_min_max(
        rect.min,
        pos2(rect.left() + rect.width() * f(db), rect.bottom()),
    );
    let color = if heard { level_color(db) } else { t.text_faint };
    p.rect_filled(fill, CornerRadius::same(2), color);
    if peak_db > METER_FLOOR_DB {
        let x = rect.left() + rect.width() * f(peak_db);
        p.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            (1.5, t.text_strong),
        );
    }
}

/// A slider `width` wide.
fn sized_slider(ui: &mut Ui, t: &Tokens, width: f32, value: &mut f32, hi: f32) -> egui::Response {
    ui.allocate_ui(vec2(width, t.row_h - 4.0), |ui| {
        crate::widgets::slider(ui, value, 0.0..=hi, 2, "")
    })
    .inner
}

/// A small toggle button (M, S, Keep).
fn toggle(ui: &mut Ui, t: &Tokens, label: &str, on: bool, on_color: Color32) -> egui::Response {
    let g = ui.painter().layout_no_wrap(label.into(), t.body(), t.text);
    let (rect, resp) = ui.allocate_exact_size(
        vec2(g.size().x.max(10.0) + 12.0, t.row_h - 6.0),
        Sense::click(),
    );
    let bg = if on {
        on_color
    } else if resp.hovered() {
        t.field_hover
    } else {
        t.field
    };
    ui.painter()
        .rect_filled(rect, CornerRadius::same(t.radius), bg);
    let col = if on { Color32::BLACK } else { t.text };
    ui.painter()
        .text(rect.center(), Align2::CENTER_CENTER, label, t.body(), col);
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, label));
    resp
}

const MUTE_COLOR: Color32 = Color32::from_rgb(232, 140, 72);
const SOLO_COLOR: Color32 = Color32::from_rgb(226, 200, 72);

impl MixerPanel {
    /// The shown level for `key`: rises at once, falls at
    /// [`METER_FALL_DB_PER_S`].
    fn smooth(&mut self, key: String, db: f32, dt: f32) -> f32 {
        let db = if db.is_finite() { db } else { METER_FLOOR_DB };
        let e = self.shown.entry(key).or_insert(db);
        *e = db.max(*e - METER_FALL_DB_PER_S * dt).max(METER_FLOOR_DB);
        *e
    }
}

/// One playing voice as the Mixer shows it.
struct Row {
    voice: VoiceInfo,
    name: String,
    uid: Option<Uid>,
    distance: Option<f32>,
    attenuation: f32,
    pan: f32,
}

enum Change {
    Fader(&'static str, f32),
    Solo(MixTarget, bool),
    Mute(MixTarget, bool),
    Clear,
    Volume(Uid, f32),
    VolumeStart,
    VolumeEnd,
    Keep(Uid, bool),
}

fn bus_label(bus: AudioBus) -> &'static str {
    match bus {
        AudioBus::Effects => "Effects",
        AudioBus::Ambience => "Ambience",
        AudioBus::Music => "Music",
    }
}

fn bus_field(bus: AudioBus) -> &'static str {
    match bus {
        AudioBus::Effects => "AudioSettings.effects",
        AudioBus::Ambience => "AudioSettings.ambience",
        AudioBus::Music => "AudioSettings.music",
    }
}

/// The Mixer window, while open.
pub(crate) fn mixer_window(ctx: &Context, state: &mut EditorState, t: &Tokens) {
    if !state.mixer.open {
        return;
    }
    let (dt, now) = ctx.input(|i| (i.stable_dt.min(0.1), i.time));
    let playing = state.is_playing();
    let settings = state
        .world
        .get_resource::<AudioSettings>()
        .cloned()
        .unwrap_or_default();
    let audio = if playing {
        state.world.get_resource::<Audio>()
    } else {
        None
    };
    let mut rows: Vec<Row> = audio
        .map(|a| {
            a.voices()
                .map(|v| {
                    let s = v.spatial(a.listener());
                    Row {
                        voice: v.clone(),
                        name: audio_mixer::voice_label(&state.world, v),
                        uid: v.source.and_then(|e| state.world.uid(e)),
                        distance: s.map(|s| s.distance),
                        attenuation: s.map_or(1.0, |s| s.attenuation),
                        pan: s.map_or(0.0, |s| s.pan),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let loud = |r: &Row| r.voice.level.map_or(-1.0, |l| l.rms());
    rows.sort_by(|a, b| {
        loud(b)
            .total_cmp(&loud(a))
            .then(a.voice.id.cmp(&b.voice.id))
    });
    let output = audio.map(|a| a.output());
    let mix = audio.map(|a| a.mix().clone()).unwrap_or_default();
    let mix_labels: Vec<(String, MixTarget, bool)> = mix
        .soloed
        .iter()
        .map(|m| (audio_mixer::target_label(&state.world, m), m.clone(), true))
        .chain(
            mix.muted
                .iter()
                .map(|m| (audio_mixer::target_label(&state.world, m), m.clone(), false)),
        )
        .collect();
    let selection: Vec<MixTarget> = state
        .selection
        .iter()
        .filter_map(|u| state.world.entity_by_uid(*u))
        .map(MixTarget::Entity)
        .collect();
    let kept: Vec<bool> = rows
        .iter()
        .map(|r| {
            r.uid
                .is_some_and(|u| state.is_kept(u, "AudioSource.volume"))
        })
        .collect();

    let mut changes: Vec<Change> = Vec::new();
    let mut open = true;
    let panel = &mut state.mixer;
    egui::Window::new("Mixer")
        .id(egui::Id::unique("box_editor_mixer"))
        .open(&mut open)
        .collapsible(false)
        .fade_in(false)
        .fade_out(false)
        .resizable(true)
        .default_width(720.0)
        .default_pos(crate::assistant::start_at(ctx, Align2::RIGHT_BOTTOM, egui::vec2(-340.0 - 720.0, -300.0 - 360.0)))
        .show(ctx, |ui| {
            let status = match output {
                None => "Not playing: the faders set the project's bus volumes. Sources, levels, \
                         solo and mute appear during Play."
                    .to_string(),
                Some(OutputKind::Silent) => "Play world · silent output: no levels to measure \
                                              (no sound device)."
                    .into(),
                Some(OutputKind::NotStarted | OutputKind::Opening) => {
                    "Play world · waiting for the first sound.".into()
                }
                Some(k) => format!(
                    "Play world · {} output · levels at the listener, after distance, bus and \
                     master volume. Solo and mute last until Stop.",
                    if k == OutputKind::Device { "device" } else { "capture" }
                ),
            };
            ui.label(RichText::new(status).color(t.text_weak).small());
            ui.add_space(6.0);

            // Buses.
            egui::Grid::new("box_editor_mixer_buses")
                .num_columns(6)
                .spacing(vec2(10.0, 4.0))
                .show(ui, |ui| {
                    let mut master = settings.master;
                    ui.label(RichText::new("Master").color(t.text_strong));
                    if sized_slider(ui, t, 140.0, &mut master, 2.0)
                        .on_hover_text("Master volume (AudioSettings.master)")
                        .changed()
                    {
                        changes.push(Change::Fader("AudioSettings.master", master));
                    }
                    let total = rows
                        .iter()
                        .filter(|r| r.voice.audible)
                        .filter_map(|r| r.voice.level)
                        .map(|l| l.rms() * l.rms())
                        .sum::<f32>()
                        .sqrt();
                    let shown = panel.smooth("master".into(), amplitude_db(total), dt);
                    meter(ui, t, 140.0, shown, METER_FLOOR_DB, true);
                    ui.label(RichText::new(db_text(total)).font(t.mono()).color(t.text));
                    ui.label("");
                    ui.label("");
                    ui.end_row();
                    for bus in AudioBus::ALL {
                        let mut v = settings.bus(bus);
                        ui.label(RichText::new(bus_label(bus)).color(t.text));
                        if sized_slider(ui, t, 140.0, &mut v, 2.0)
                            .on_hover_text(format!("{} bus volume ({})", bus_label(bus), bus_field(bus)))
                            .changed()
                        {
                            changes.push(Change::Fader(bus_field(bus), v));
                        }
                        let on_bus: Vec<&Row> = rows.iter().filter(|r| r.voice.bus == bus).collect();
                        let level = on_bus
                            .iter()
                            .filter(|r| r.voice.audible)
                            .filter_map(|r| r.voice.level)
                            .map(|l| l.rms() * l.rms())
                            .sum::<f32>()
                            .sqrt();
                        let shown = panel.smooth(format!("bus:{bus:?}"), amplitude_db(level), dt);
                        meter(ui, t, 140.0, shown, METER_FLOOR_DB, true);
                        ui.label(RichText::new(db_text(level)).font(t.mono()).color(t.text));
                        let target = MixTarget::Bus(bus);
                        let (m, s) = (mix.muted.contains(&target), mix.soloed.contains(&target));
                        ui.add_enabled_ui(playing, |ui| {
                            ui.horizontal(|ui| {
                                if toggle(ui, t, "M", m, MUTE_COLOR)
                                    .on_hover_text("Mute the bus (until Stop)")
                                    .clicked()
                                {
                                    changes.push(Change::Mute(target.clone(), !m));
                                }
                                if toggle(ui, t, "S", s, SOLO_COLOR)
                                    .on_hover_text("Solo the bus (until Stop)")
                                    .clicked()
                                {
                                    changes.push(Change::Solo(target.clone(), !s));
                                }
                            });
                        });
                        ui.label(RichText::new(format!("{} playing", on_bus.len())).color(t.text_weak).small());
                        ui.end_row();
                    }
                });

            if !playing {
                return;
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!selection.is_empty(), |ui| {
                    if crate::widgets::text_button(ui, "Solo selected", t.row_h - 4.0, false)
                        .on_hover_text("Solo the selected entities and the sources below them (a group)")
                        .clicked()
                    {
                        for s in &selection {
                            changes.push(Change::Solo(s.clone(), true));
                        }
                    }
                    if crate::widgets::text_button(ui, "Mute selected", t.row_h - 4.0, false)
                        .on_hover_text("Mute the selected entities and the sources below them")
                        .clicked()
                    {
                        for s in &selection {
                            changes.push(Change::Mute(s.clone(), true));
                        }
                    }
                });
                ui.add_enabled_ui(!mix_labels.is_empty(), |ui| {
                    if crate::widgets::text_button(ui, "Clear solo and mute", t.row_h - 4.0, false)
                        .clicked()
                    {
                        changes.push(Change::Clear);
                    }
                });
            });
            if !mix_labels.is_empty() {
                ui.horizontal_wrapped(|ui| {
                    for (label, target, solo) in &mix_labels {
                        let text = format!("{} {label}  ×", if *solo { "Solo" } else { "Mute" });
                        if toggle(ui, t, &text, true, if *solo { SOLO_COLOR } else { MUTE_COLOR })
                            .on_hover_text("Remove")
                            .clicked()
                        {
                            changes.push(if *solo {
                                Change::Solo(target.clone(), false)
                            } else {
                                Change::Mute(target.clone(), false)
                            });
                        }
                    }
                });
            }
            ui.add_space(6.0);
            if rows.is_empty() {
                ui.label(RichText::new("Nothing is playing.").color(t.text_weak));
                return;
            }
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                egui::Grid::new("box_editor_mixer_sources")
                    .striped(true)
                    .num_columns(9)
                    .spacing(vec2(10.0, 4.0))
                    .show(ui, |ui| {
                        for h in ["Source", "Bus", "Distance", "Atten.", "Pan", "Level", "", "Volume", ""] {
                            ui.label(RichText::new(h).color(t.text_weak).small());
                        }
                        ui.end_row();
                        for (row, kept) in rows.iter().zip(&kept) {
                            let v = &row.voice;
                            let name = RichText::new(&row.name).color(if v.audible {
                                t.text_strong
                            } else {
                                t.text_faint
                            });
                            ui.label(name).on_hover_text(format!(
                                "{}{}",
                                v.clip.path.as_deref().unwrap_or(""),
                                if v.source.is_none() { " (one-shot)" } else { "" }
                            ));
                            ui.label(RichText::new(bus_label(v.bus)).color(t.text));
                            ui.label(
                                RichText::new(row.distance.map_or("—".into(), |d| format!("{d:.1} m")))
                                    .font(t.mono())
                                    .color(t.text),
                            )
                            .on_hover_text(if v.position.is_some() {
                                format!("Full volume within {:.1} m, silent beyond {:.1} m", v.min_distance, v.max_distance)
                            } else {
                                "Not positional: heard everywhere".into()
                            });
                            ui.label(RichText::new(db_text(row.attenuation)).font(t.mono()).color(t.text))
                                .on_hover_text("Distance attenuation");
                            let pan = if row.pan.abs() < 0.05 {
                                "C".to_string()
                            } else if row.pan < 0.0 {
                                format!("L{:.0}", -row.pan * 100.0)
                            } else {
                                format!("R{:.0}", row.pan * 100.0)
                            };
                            ui.label(RichText::new(pan).font(t.mono()).color(t.text));
                            let (rms, peak) = v.level.map_or((0.0, 0.0), |l| (l.rms(), l.peak));
                            let shown = panel.smooth(format!("v{}", v.id), amplitude_db(rms), dt);
                            meter(ui, t, 140.0, shown, amplitude_db(peak), v.audible);
                            ui.label(RichText::new(db_text(rms)).font(t.mono()).color(t.text));
                            match row.uid {
                                Some(uid) => {
                                    let mut vol = v.volume;
                                    let r = sized_slider(ui, t, 110.0, &mut vol, 2.0)
                                        .on_hover_text("AudioSource.volume (a Play edit: Keep applies it to the scene at Stop)");
                                    if r.drag_started() {
                                        changes.push(Change::VolumeStart);
                                    }
                                    if r.changed() {
                                        changes.push(Change::Volume(uid, vol));
                                    }
                                    if r.drag_stopped() {
                                        changes.push(Change::VolumeEnd);
                                    }
                                }
                                None => {
                                    ui.label(RichText::new(format!("{:.2}", v.volume)).font(t.mono()).color(t.text_weak));
                                }
                            }
                            let target = match v.source {
                                Some(e) => MixTarget::Entity(e),
                                None => MixTarget::Clip(v.clip.clone()),
                            };
                            let (m, s) = (mix.muted.contains(&target), mix.soloed.contains(&target));
                            ui.horizontal(|ui| {
                                if toggle(ui, t, "M", m || v.muted, MUTE_COLOR)
                                    .on_hover_text(if v.muted && !m {
                                        "Muted through a group, bus or clip"
                                    } else {
                                        "Mute (until Stop)"
                                    })
                                    .clicked()
                                {
                                    changes.push(Change::Mute(target.clone(), !m));
                                }
                                if toggle(ui, t, "S", s || v.soloed, SOLO_COLOR)
                                    .on_hover_text("Solo (until Stop)")
                                    .clicked()
                                {
                                    changes.push(Change::Solo(target.clone(), !s));
                                }
                                if let Some(uid) = row.uid
                                    && toggle(ui, t, "Keep", *kept, t.accent)
                                        .on_hover_text("Keep this volume after Stop (applied to the scene)")
                                        .clicked()
                                {
                                    changes.push(Change::Keep(uid, !kept));
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
        });
    if !open {
        state.mixer.open = false;
    }
    for c in changes {
        match c {
            Change::Fader(path, v) => {
                state.set_settings_field("AudioSettings", path, &v, now);
            }
            Change::Solo(target, on) => {
                state.audio_solo(target, on);
            }
            Change::Mute(target, on) => {
                state.audio_mute(target, on);
            }
            Change::Clear => state.audio_clear_mix(),
            Change::VolumeStart => {
                state.begin_edit("Mixer volume");
                state.mixer.dragging_volume = true;
            }
            Change::Volume(uid, v) => {
                state.set_field(uid, "AudioSource.volume", &v);
            }
            Change::VolumeEnd => {
                if std::mem::take(&mut state.mixer.dragging_volume) {
                    state.finish_edit();
                }
            }
            Change::Keep(uid, on) => {
                if on {
                    state.keep_after_stop(&[uid], "AudioSource.volume");
                } else {
                    state.forget_kept(&[uid], "AudioSource.volume");
                }
            }
        }
    }
    // Repaint while sound plays, so the meters move.
    if !rows.is_empty() {
        ctx.request_repaint();
    }
}

/// Colour of ranges while editing (no levels yet).
const RANGE_EDIT: [f32; 3] = [0.45, 0.75, 1.0];
/// Segments of a range ring.
const RING_SEGMENTS: usize = 64;

fn to_color(c: Color32, alpha: f32) -> box_core::color::Color {
    box_core::color::Color::srgb(
        c.r() as f32 / 255.0,
        c.g() as f32 / 255.0,
        c.b() as f32 / 255.0,
    )
    .with_alpha(alpha)
}

/// A horizontal ring of `radius` around `centre`.
fn ring(
    dd: &mut box_render::DebugDraw,
    layer: box_render::LineLayer,
    centre: glam::Vec3,
    radius: f32,
    color: box_core::color::Color,
) {
    if radius <= 0.0 {
        return;
    }
    let at = |i: usize| {
        let a = i as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
        centre + glam::Vec3::new(a.cos(), 0.0, a.sin()) * radius
    };
    for i in 0..RING_SEGMENTS {
        dd.line_on(layer, at(i), at(i + 1), color);
    }
}

/// View ▸ Audio Ranges: each positional sound's full-volume distance (inner
/// ring) and silent distance (outer ring) around it, horizontal at its
/// height. While editing, every spatial `AudioSource`, in blue. During Play,
/// every playing positional voice (one-shots too), coloured by its level at
/// the listener (green, yellow above -12 dB, red above -3 dB; dark grey
/// when silent, out of range, muted or soloed out), with a line to the
/// listener and a cross at the listener. Without measured levels (a silent output)
/// the colour follows the expected gain (volume, distance, bus and master).
///
/// Cost: a few dozen lines per source, only while the view is on.
pub(crate) fn draw_audio_ranges(state: &EditorState, dd: &mut box_render::DebugDraw) {
    use box_core::transform::GlobalTransform;
    use box_engine::box_audio::AudioSource;
    use box_render::LineLayer;
    if !state.viewport.view.audio {
        return;
    }
    let world = &state.world;
    let audio = if state.is_playing() {
        world.get_resource::<Audio>()
    } else {
        None
    };
    let Some(audio) = audio else {
        let [r, g, b] = RANGE_EDIT;
        let color = box_core::color::Color::linear(r, g, b);
        for (_, src, gt) in world
            .query::<(box_core::world::Entity, &AudioSource, &GlobalTransform)>()
            .iter()
        {
            if !src.spatial {
                continue;
            }
            let p: glam::Vec3 = gt.0.translation.into();
            ring(dd, LineLayer::Display, p, src.min_distance, color);
            ring(
                dd,
                LineLayer::Display,
                p,
                src.max_distance,
                color.with_alpha(0.45),
            );
        }
        return;
    };
    let settings = world
        .get_resource::<AudioSettings>()
        .cloned()
        .unwrap_or_default();
    let (listener, rot) = audio.listener();
    let measured = !matches!(audio.output(), OutputKind::Silent | OutputKind::NotStarted);
    let grey = to_color(Color32::from_gray(40), 0.75);
    let mut any = false;
    for v in audio.voices() {
        let Some(s) = v.spatial((listener, rot)) else {
            continue;
        };
        let Some(p) = v.position else {
            continue;
        };
        any = true;
        let level = if measured {
            v.level.map_or(0.0, |l| l.rms())
        } else {
            v.volume * s.attenuation * settings.bus(v.bus) * settings.master
        };
        let db = amplitude_db(level);
        let heard = v.audible && db > METER_FLOOR_DB;
        let color = if heard {
            to_color(level_color(db), 1.0)
        } else {
            grey
        };
        ring(dd, LineLayer::Display, p, v.min_distance, color);
        ring(
            dd,
            LineLayer::Display,
            p,
            v.max_distance,
            color.with_alpha(color.a * 0.4),
        );
        dd.line_on(
            LineLayer::DisplayOverlay,
            p,
            listener,
            color.with_alpha(color.a * 0.8),
        );
    }
    if any {
        let c = to_color(Color32::WHITE, 0.9);
        for axis in [glam::Vec3::X, glam::Vec3::Y, glam::Vec3::Z] {
            dd.line_on(
                LineLayer::DisplayOverlay,
                listener - axis * 0.25,
                listener + axis * 0.25,
                c,
            );
        }
    }
}

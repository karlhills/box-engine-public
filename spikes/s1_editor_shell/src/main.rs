//! Spike S1: egui 0.37 + egui_tiles editor shell.
//!
//!   cargo run --release                 # windowed
//!   cargo run --release -- --capture    # writes captures/*.png and layout JSON
//!   cargo run --release -- --bench      # headless frame timings -> captures/bench.json

mod app;
mod gfx;
mod headless;
mod scene;
mod theme;
mod viewport;
mod widgets;
mod windowed;

use app::{Pane, Shell};
use egui::{Event, pos2};
use headless::{FrameTiming, Headless};
use std::path::Path;

const SIZE: [u32; 2] = [2560, 1440];
const PPP: f32 = 2.0;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--capture") {
        capture()
    } else if args.iter().any(|a| a == "--bench") {
        let frames = args
            .iter()
            .position(|a| a == "--frames")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(300);
        bench(frames)
    } else {
        let seconds = args
            .iter()
            .position(|a| a == "--seconds")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse().ok());
        windowed::run(seconds)
    }
}

/// Round-trips a layout through JSON on disk and returns the restored tree.
fn roundtrip(tree: egui_tiles::Tree<Pane>, path: &Path) -> anyhow::Result<egui_tiles::Tree<Pane>> {
    app::save_layout(path, &tree)?;
    app::load_layout(path)
}

fn settle(h: &mut Headless, n: usize) {
    for _ in 0..n {
        h.frame(vec![Event::PointerMoved(pos2(900.0, 300.0))]);
    }
}

fn capture() -> anyhow::Result<()> {
    let dir = Path::new("captures");

    // 1. Default layout, an entity selected.
    let tree = roundtrip(app::default_layout(), &dir.join("layout_default.json"))?;
    let mut h = Headless::new(Shell::new(tree), SIZE, PPP)?;
    println!("adapter: {}", h.adapter_name);
    let ed = &mut h.shell.ed;
    let sel = ed.find("Townhouse_013").expect("entity");
    ed.reveal_and_select(sel);
    ed.expanded.insert(sel);
    if let Some(&wa) = ed.scene.entities[sel as usize]
        .children
        .iter()
        .find(|&&c| ed.scene.entities[c as usize].name == "Window Array")
    {
        ed.expanded.insert(wa);
    }
    ed.rows_dirty = true;
    settle(&mut h, 4);
    h.capture(&dir.join("shell_dark.png"))?;

    // 2. Deep tree with a search active.
    let ed = &mut h.shell.ed;
    ed.set_expand_all(true);
    ed.filter = "window 03".into();
    ed.rows_dirty = true;
    settle(&mut h, 2);
    let ed = &mut h.shell.ed;
    if let Some(r) = ed
        .rows
        .iter()
        .skip(40)
        .find(|r| ed.scene.entities[r.id as usize].name == "Window 03")
    {
        let id = r.id;
        ed.select(id);
    }
    ed.hier_scroll_to = Some(28.0 * theme::tokens().row_h);
    settle(&mut h, 3);
    h.capture(&dir.join("shell_hierarchy_deep.png"))?;

    // 3. Alternative docked layout, restored from JSON.
    let tree = roundtrip(app::alt_layout(), &dir.join("layout_alt.json"))?;
    let mut h = Headless::new(Shell::new(tree), SIZE, PPP)?;
    let ed = &mut h.shell.ed;
    let sel = ed.find("Lamp Post 03").expect("entity");
    ed.reveal_and_select(sel);
    settle(&mut h, 4);
    h.capture(&dir.join("shell_layout_alt.png"))?;

    // 4. Same default layout with the Assets tab in front (bottom tab group).
    let tree = roundtrip(app::default_layout(), &dir.join("layout_default.json"))?;
    let mut h = Headless::new(Shell::new(tree), SIZE, PPP)?;
    h.shell.activate(Pane::Assets);
    let ed = &mut h.shell.ed;
    let sel = ed.find("Sun").expect("entity");
    ed.reveal_and_select(sel);
    settle(&mut h, 4);
    h.capture(&dir.join("shell_assets_tab.png"))?;

    let errs = h.errors.load(std::sync::atomic::Ordering::Relaxed);
    println!("captures written to {}; wgpu errors: {errs}", dir.display());
    anyhow::ensure!(errs == 0, "wgpu validation errors");
    Ok(())
}

#[derive(serde::Serialize)]
struct Stat {
    mean: f32,
    p95: f32,
    max: f32,
}

fn stat(v: &[f32]) -> Stat {
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    Stat {
        mean: s.iter().sum::<f32>() / s.len() as f32,
        p95: s[((s.len() as f32 * 0.95) as usize).min(s.len() - 1)],
        max: s[s.len() - 1],
    }
}

#[derive(serde::Serialize)]
struct Scenario {
    name: String,
    frames: usize,
    rows: usize,
    flatten_ms: f32,
    ui_ms: Stat,
    tess_ms: Stat,
    prep_ms: Stat,
    submit_wait_ms: Stat,
    gpu_viewport_ms: Stat,
    gpu_egui_ms: Stat,
    total_ms: Stat,
}

fn summarise(name: &str, h: &Headless, t: &[FrameTiming], flatten_ms: f32) -> Scenario {
    let s = Scenario {
        name: name.into(),
        frames: t.len(),
        rows: h.shell.ed.rows.len(),
        flatten_ms,
        ui_ms: stat(&t.iter().map(|x| x.ui_ms).collect::<Vec<_>>()),
        tess_ms: stat(&t.iter().map(|x| x.tess_ms).collect::<Vec<_>>()),
        prep_ms: stat(&t.iter().map(|x| x.prep_ms).collect::<Vec<_>>()),
        submit_wait_ms: stat(&t.iter().map(|x| x.gpu_ms).collect::<Vec<_>>()),
        gpu_viewport_ms: stat(&t.iter().map(|x| x.gpu_viewport_ms).collect::<Vec<_>>()),
        gpu_egui_ms: stat(&t.iter().map(|x| x.gpu_egui_ms).collect::<Vec<_>>()),
        total_ms: stat(&t.iter().map(|x| x.total_ms).collect::<Vec<_>>()),
    };
    let f = |x: &Stat| format!("{:>5.2}/{:>5.2}/{:>5.2}", x.mean, x.p95, x.max);
    println!(
        "{:<29} rows {:>5} | ui {} | tess {} | prep {} | submit+wait {} | gpu vp {} | gpu egui {} | total {}",
        s.name,
        s.rows,
        f(&s.ui_ms),
        f(&s.tess_ms),
        f(&s.prep_ms),
        f(&s.submit_wait_ms),
        f(&s.gpu_viewport_ms),
        f(&s.gpu_egui_ms),
        f(&s.total_ms)
    );
    s
}

fn bench(frames: usize) -> anyhow::Result<()> {
    let mut h = Headless::new(Shell::new(app::default_layout()), SIZE, PPP)?;
    println!(
        "adapter: {}  target {}x{} px @ {}x  frames/scenario {}",
        h.adapter_name, SIZE[0], SIZE[1], PPP, frames
    );
    let sel = h.shell.ed.find("Townhouse_013").expect("entity");
    h.shell.ed.reveal_and_select(sel);
    settle(&mut h, 30); // warm-up: font atlas, pipelines, caches
    let mut out = Vec::new();

    // Hover position inside the hierarchy pane (points).
    let hier_x = 120.0;
    let hover = |i: usize| Event::PointerMoved(pos2(hier_x, 120.0 + (i % 400) as f32 * 1.1));

    // A. Default view, a few hundred rows, pointer moving over the hierarchy.
    let t: Vec<_> = (0..frames).map(|i| h.frame(vec![hover(i)])).collect();
    out.push(summarise("default", &h, &t, h.shell.ed.last_flatten_ms));

    // B. Whole tree expanded (~10k rows), scrolling to a new position every frame.
    h.shell.ed.set_expand_all(true);
    h.frame(vec![]);
    let flatten = h.shell.ed.last_flatten_ms;
    let n = h.shell.ed.rows.len() as f32;
    let max_off = n * theme::tokens().row_h;
    let t: Vec<_> = (0..frames)
        .map(|i| {
            // sweep down with large jumps, then wrap: visits positions across the whole list
            let f = (i as f32 * 0.137).fract();
            h.shell.ed.hier_scroll_to = Some(f * max_off);
            h.frame(vec![hover(i)])
        })
        .collect();
    out.push(summarise("expanded_10k_jump_scroll", &h, &t, flatten));

    // C. Expanded, smooth wheel-like scrolling (small steps).
    let t: Vec<_> = (0..frames)
        .map(|i| {
            h.shell.ed.hier_scroll_to = Some(2000.0 + i as f32 * 24.0);
            h.frame(vec![hover(i)])
        })
        .collect();
    out.push(summarise("expanded_10k_smooth_scroll", &h, &t, flatten));

    // D. Search active over the expanded tree.
    h.shell.ed.filter = "a".into();
    h.shell.ed.rows_dirty = true;
    h.frame(vec![]);
    let flatten = h.shell.ed.last_flatten_ms;
    let n = h.shell.ed.rows.len() as f32;
    let t: Vec<_> = (0..frames)
        .map(|i| {
            let f = (i as f32 * 0.071).fract();
            h.shell.ed.hier_scroll_to = Some(f * n * theme::tokens().row_h);
            h.frame(vec![hover(i)])
        })
        .collect();
    out.push(summarise("search_a_scroll", &h, &t, flatten));

    // E. Re-flatten every frame (worst case: expand/collapse or typing each frame).
    h.shell.ed.filter.clear();
    let t: Vec<_> = (0..frames)
        .map(|i| {
            h.shell.ed.rows_dirty = true;
            h.frame(vec![hover(i)])
        })
        .collect();
    let fl = h.shell.ed.last_flatten_ms;
    out.push(summarise("expanded_reflatten_each_frame", &h, &t, fl));

    // F/G. Same as A and B with the viewport pass skipped: egui's own CPU+GPU cost.
    h.gfx.skip_viewport = true;
    h.shell.ed.set_expand_all(false);
    h.shell.ed.reveal_and_select(sel);
    h.frame(vec![]);
    let t: Vec<_> = (0..frames).map(|i| h.frame(vec![hover(i)])).collect();
    out.push(summarise(
        "default_ui_only",
        &h,
        &t,
        h.shell.ed.last_flatten_ms,
    ));
    h.shell.ed.set_expand_all(true);
    h.frame(vec![]);
    let flatten = h.shell.ed.last_flatten_ms;
    let t: Vec<_> = (0..frames)
        .map(|i| {
            let f = (i as f32 * 0.137).fract();
            h.shell.ed.hier_scroll_to = Some(f * max_off);
            h.frame(vec![hover(i)])
        })
        .collect();
    out.push(summarise("expanded_10k_jump_ui_only", &h, &t, flatten));

    let errs = h.errors.load(std::sync::atomic::Ordering::Relaxed);
    std::fs::create_dir_all("captures")?;
    std::fs::write(
        "captures/bench.json",
        serde_json::to_string_pretty(&serde_json::json!({
            "adapter": h.adapter_name,
            "size_px": SIZE,
            "pixels_per_point": PPP,
            "frames_per_scenario": frames,
            "wgpu_errors": errs,
            "scenarios": out,
        }))?,
    )?;
    println!("wrote captures/bench.json; wgpu errors: {errs}");
    Ok(())
}

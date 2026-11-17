//! Golden image of the whole editor (default layout without a project, so
//! the Console is in front; scratch scene, one entity selected), rendered
//! through the headless capture path.
//!
//! Skipped when `BOX_SKIP_GPU_TESTS` is set or no GPU adapter exists.
//! Re-record with `BOX_UPDATE_GOLDEN=1 cargo test -p box_editor --test golden`
//! and review the PNG. References are recorded on Apple Silicon (Metal).

use std::path::PathBuf;

use box_editor::headless::{CaptureOptions, capture, select_sample_entity};
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, build_editor, layout};
use box_render::{RenderError, golden};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;

#[test]
fn editor_default_layout() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        eprintln!("BOX_SKIP_GPU_TESTS set; skipping the editor golden");
        return;
    }
    // Capture this thread's editor log records into the console, with the
    // frame clock. Renderer records carry machine paths and the adapter name.
    let log = LogBuffer::default();
    log.set_manual_time(0.0);
    let layer = log
        .layer()
        .with_filter(filter_fn(|m| m.target().starts_with("box_editor")));
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));

    let mut editor = build_editor(&EditorOptions::default(), log).expect("editor");
    // The default without a project: the Console in front of Assets.
    editor.tree = layout::default_layout_for(false);
    select_sample_entity(&mut editor);
    let options = CaptureOptions {
        size: [1280, 720],
        pixels_per_point: 1.0,
        frames: 4,
    };
    let image = match capture(&mut editor, &options) {
        Ok(image) => image,
        Err(e)
            if matches!(
                e.downcast_ref::<RenderError>(),
                Some(RenderError::NoAdapter)
            ) =>
        {
            eprintln!("no GPU adapter; skipping the editor golden");
            return;
        }
        Err(e) => panic!("{e:#}"),
    };
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    image.save(out.join("editor_default.png")).unwrap();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    if let Err(e) = golden::check(&dir, "editor_default", &image) {
        panic!("{e}");
    }
}

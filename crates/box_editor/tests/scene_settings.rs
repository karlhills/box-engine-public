//! Capture of the scene-settings inspector (nothing selected, so the
//! inspector edits the scene's `Environment` resource). Written to
//! target/captures for review; not a golden.
//!
//! Kept in its own test binary: it builds an editor without a log
//! subscriber, and in the same process that races the golden test's
//! thread-local subscriber for tracing's global callsite interest cache,
//! which dropped the golden's console line when the tests ran in parallel.

use std::path::PathBuf;

use box_editor::headless::{CaptureOptions, capture};
use box_editor::log::LogBuffer;
use box_editor::{EditorOptions, build_editor, layout};
use box_render::RenderError;

#[test]
fn scene_settings_capture() {
    if std::env::var_os("BOX_SKIP_GPU_TESTS").is_some() {
        return;
    }
    let mut editor = build_editor(&EditorOptions::default(), LogBuffer::default()).expect("editor");
    editor.tree = layout::default_layout();
    let sections = box_editor::inspect::resource_sections(&editor.state.world);
    assert!(sections.iter().any(|s| s.component == "Environment"));
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
            return;
        }
        Err(e) => panic!("{e:#}"),
    };
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/captures");
    std::fs::create_dir_all(&out).unwrap();
    image.save(out.join("editor_scene_settings.png")).unwrap();
}

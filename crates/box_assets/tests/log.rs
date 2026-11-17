//! The error log of a failed import. Its own test binary (process): tracing
//! caches callsite interest globally, so a scoped subscriber in a test
//! running in parallel with others can miss events.

mod common;

use std::sync::Arc;

use box_core::asset_ref::AssetRef;
use box_core::assets::Handle;
use box_core::image::ImageData;
use box_core::world::World;
use common::{TempAssets, settle};

fn r(path: &str) -> AssetRef {
    AssetRef::path(path)
}

/// A tracing writer that keeps everything written to it.
#[derive(Clone, Default)]
struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn failed_import_logs_an_error() {
    let assets = TempAssets::new("log");
    let server = assets.server();
    let mut world = World::new();
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let _: Handle<ImageData> = server.load(&r("bad/truncated.png")).unwrap();
        settle(&server, &mut world);
    });
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(
        text.contains("ERROR") && text.contains("bad/truncated.png: import failed"),
        "{text}"
    );
}

/// A successful re-import (hot reload) is announced at info level with its
/// duration; the first import is not.
#[test]
fn reload_logs_an_info_line() {
    let assets = TempAssets::new("reload_log");
    let server = assets.server();
    let mut world = World::new();
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let _: Handle<ImageData> = server.load(&r("textures/checker.png")).unwrap();
        settle(&server, &mut world);
        assert!(!String::from_utf8_lossy(&log.0.lock().unwrap()).contains("reloaded"));
        let id = server.id_of("textures/checker.png").unwrap();
        server.reimport(id).unwrap();
        settle(&server, &mut world);
    });
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(
        text.contains("INFO") && text.contains("reloaded textures/checker.png ("),
        "{text}"
    );
}

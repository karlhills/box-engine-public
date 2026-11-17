//! Hot reload through the file watcher (notify + debouncer).

mod common;

use std::time::{Duration, Instant};

use box_assets::{AssetEvent, AssetEventKind, AssetServer, LoadState};
use box_core::asset_ref::AssetRef;
use box_core::assets::{AssetLookup, Assets, Handle};
use box_core::event::{EventCursor, Events};
use box_core::image::{ColorSpace, ImageData};
use box_core::world::World;
use common::{TempAssets, settle};

/// The bound on how long a change may take to show up.
const WAIT: Duration = Duration::from_secs(10);

/// Updates until an event of `kind` for `h` arrives; returns the wait.
fn wait_for<T: 'static>(
    server: &AssetServer,
    world: &mut World,
    cursor: &mut EventCursor<AssetEvent>,
    h: Handle<T>,
    kind: AssetEventKind,
) -> Duration {
    let start = Instant::now();
    while start.elapsed() < WAIT {
        server.update(world);
        if let Some(events) = world.get_resource::<Events<AssetEvent>>()
            && cursor.read(events).any(|e| e.is(h) && e.kind == kind)
        {
            return start.elapsed();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("no {kind:?} event within {WAIT:?}");
}

#[test]
fn modified_deleted_and_moved_files_reload_in_place() {
    let assets = TempAssets::new("hot");
    let server = assets.server();
    server.watch(true).unwrap();
    assert!(server.is_watching());
    let mut world = World::new();
    let mut cursor = EventCursor::<AssetEvent>::default();
    let tex: Handle<ImageData> = server
        .load(&AssetRef::path("textures/checker.png"))
        .unwrap();
    wait_for(
        &server,
        &mut world,
        &mut cursor,
        tex,
        AssetEventKind::Loaded,
    );
    assert_eq!(
        &world.resource::<Assets<ImageData>>().get(tex).unwrap().mips[0][..4],
        &[200, 40, 40, 255]
    );

    // 1. New pixels, written the way many tools save: temp file + rename.
    let tmp = assets.path("textures/checker.png.tmp~");
    std::fs::write(&tmp, common::png(8, 8, |_, _| [10, 20, 30, 255])).unwrap();
    std::fs::rename(&tmp, assets.path("textures/checker.png")).unwrap();
    let waited = wait_for(
        &server,
        &mut world,
        &mut cursor,
        tex,
        AssetEventKind::Reloaded,
    );
    let img = world.resource::<Assets<ImageData>>().get(tex).unwrap();
    assert_eq!(
        (img.width, &img.mips[0][..4]),
        (8, &[10u8, 20, 30, 255][..])
    );
    eprintln!("modified PNG reloaded after {waited:?}");

    // 2. A .meta settings change re-imports.
    let meta = std::fs::read_to_string(assets.path("textures/checker.png.meta")).unwrap();
    std::fs::write(
        assets.path("textures/checker.png.meta"),
        meta.replace("\"srgb\"", "\"linear\""),
    )
    .unwrap();
    wait_for(
        &server,
        &mut world,
        &mut cursor,
        tex,
        AssetEventKind::Reloaded,
    );
    assert_eq!(
        world
            .resource::<Assets<ImageData>>()
            .get(tex)
            .unwrap()
            .color_space,
        ColorSpace::Linear
    );

    // 3. Moved together with its .meta by another tool: same id and slot.
    std::fs::create_dir_all(assets.path("ui")).unwrap();
    std::fs::rename(
        assets.path("textures/checker.png.meta"),
        assets.path("ui/checker.png.meta"),
    )
    .unwrap();
    std::fs::rename(
        assets.path("textures/checker.png"),
        assets.path("ui/checker.png"),
    )
    .unwrap();
    wait_for(&server, &mut world, &mut cursor, tex, AssetEventKind::Moved);
    settle(&server, &mut world);
    assert_eq!(server.handle_state(tex), Some(LoadState::Loaded));
    assert_eq!(
        world
            .resource::<AssetLookup>()
            .get(&AssetRef::path("ui/checker.png")),
        Some(tex)
    );

    // 4. Deleted: the slot keeps the placeholder.
    std::fs::remove_file(assets.path("ui/checker.png")).unwrap();
    wait_for(
        &server,
        &mut world,
        &mut cursor,
        tex,
        AssetEventKind::Removed,
    );
    assert!(server.is_placeholder(tex));
    assert!(world.resource::<Assets<ImageData>>().get(tex).is_some());

    server.watch(false).unwrap();
    assert!(!server.is_watching());
}

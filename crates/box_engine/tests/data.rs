//! Data tables (`*.data.json`): read by path from game code, typed and
//! untyped, and hot-reloaded in place.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use box_engine::data::{data_table, data_version};

fn temp_project() -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "box_data_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(d.join("data")).unwrap();
    d
}

#[derive(serde::Deserialize, Debug, PartialEq)]
struct Enemy {
    hp: u32,
    speed: f32,
}

#[test]
fn data_tables_read_by_path_and_hot_reload() {
    let dir = temp_project();
    let file = dir.join("data/enemies.data.json");
    std::fs::write(
        &file,
        r#"[{"id": "slime", "hp": 3, "speed": 1.5}, {"id": "bat", "hp": 1, "speed": 4.0}]"#,
    )
    .unwrap();
    let mut app = box_engine::default_app();
    let server = box_engine::asset_server(&dir).unwrap();
    server.watch(true).unwrap();
    app.world.insert_resource(server);
    const PATH: &str = "data/enemies.data.json";
    assert!(
        data_table(&mut app.world, PATH).is_none(),
        "requested, not imported yet"
    );
    box_engine::settle_assets(&mut app.world, Duration::from_secs(10));
    let t = data_table(&mut app.world, PATH).expect("imported");
    assert_eq!(
        t.row_as::<Enemy>("slime").unwrap(),
        Enemy { hp: 3, speed: 1.5 }
    );
    assert_eq!(t.get("1.hp"), Some(&serde_json::json!(1)));
    assert_eq!(t.rows().len(), 2);
    let v1 = data_version(&mut app.world, PATH).unwrap();

    std::fs::write(&file, r#"[{"id": "slime", "hp": 5, "speed": 1.5}]"#).unwrap();
    let start = Instant::now();
    loop {
        app.tick(1.0 / 60.0);
        if data_version(&mut app.world, PATH) != Some(v1) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "no reload");
        std::thread::sleep(Duration::from_millis(5));
    }
    let t = data_table(&mut app.world, PATH).unwrap();
    assert_eq!(t.row_as::<Enemy>("slime").unwrap().hp, 5);
    assert!(t.row("bat").is_none());
    // A missing file: no table, no panic.
    assert!(data_table(&mut app.world, "data/none.data.json").is_none());
    let _ = std::fs::remove_dir_all(dir);
}

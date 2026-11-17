//! Regenerates the committed fixture corpus in `tests/fixtures/`:
//! `cargo test -p box_assets --test gen_fixtures -- --ignored`.
//! The default run checks that the committed files match the generator.

mod common;

#[test]
#[ignore = "rewrites tests/fixtures; run explicitly"]
fn regenerate_fixtures() {
    let dir = common::fixtures_dir();
    let _ = std::fs::remove_dir_all(&dir);
    common::write_fixtures(&dir);
}

#[test]
fn committed_fixtures_match_the_generator() {
    let tmp = std::env::temp_dir().join(format!("box_assets_gen_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    common::write_fixtures(&tmp);
    let mut files = Vec::new();
    collect(&tmp, &tmp, &mut files);
    let mut total = 0;
    for rel in &files {
        let fresh = std::fs::read(tmp.join(rel)).unwrap();
        let committed = std::fs::read(common::fixtures_dir().join(rel))
            .unwrap_or_else(|_| panic!("{rel} missing; run the ignored regenerate_fixtures test"));
        total += committed.len();
        assert!(
            fresh == committed,
            "{rel} differs from the generator; run the ignored regenerate_fixtures test"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(total < 200 * 1024, "fixtures are {total} bytes");
}

fn collect(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(root, &p, out);
        } else {
            out.push(
                p.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
}

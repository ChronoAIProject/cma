//! Byte-for-byte comparison against reviewed golden fixtures.
//!
//! A golden fixture is the recorded behaviour of the release that preceded a
//! change, and the compatibility contract the change must keep. Fixtures are
//! generated once, on the parent commit of the change that introduces them,
//! with `CMAEG_GOLDEN_UPDATE=1`, and committed after review. Without that
//! variable every comparison is exact.
//!
//! Updating a golden is a reviewed contract change, never a way to make a
//! test pass: a regenerated fixture must be justified in its own commit by a
//! deliberate, documented change to what the gateway sends.
use std::path::PathBuf;

/// Compare `actual` with the fixture at `fixtures/<relative>`, or write it
/// when `CMAEG_GOLDEN_UPDATE=1` is set.
pub fn assert_golden(relative: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/fixtures")
        .join(relative);
    if std::env::var("CMAEG_GOLDEN_UPDATE").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().expect("fixtures live in a directory"))
            .expect("golden fixture directory is writable");
        std::fs::write(&path, actual).expect("golden fixture is writable");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "golden fixture {} is unreadable ({error}); generate it on the parent \
             commit with CMAEG_GOLDEN_UPDATE=1 and review it",
            path.display()
        )
    });
    assert!(
        expected == actual,
        "golden fixture {} differs from this run.\n--- expected\n{expected}\n--- actual\n{actual}",
        path.display()
    );
}

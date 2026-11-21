//! The committed media fixtures and the self-test's cases name the same
//! files, and each fixture stays small enough to commit.
use cmaeg::media::transform::self_test::CASES;
use std::{fs, path::Path};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/media");
/// Every fixture's cap, except the 4K HEVC clip's.
const MAX_FIXTURE_BYTES: u64 = 65_536;
/// Two 3840×2160 intra frames need more room.
const MAX_HEVC_BYTES: u64 = 262_144;

fn fixture_names() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(FIXTURES)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort_unstable();
    names
}

#[test]
fn every_fixture_has_a_case_and_every_case_a_fixture() {
    let mut cases: Vec<String> = CASES.iter().map(|case| case.file.to_owned()).collect();
    cases.sort_unstable();
    assert_eq!(fixture_names(), cases);
}

#[test]
fn fixture_sizes_are_bounded() {
    for name in fixture_names() {
        let size = fs::metadata(Path::new(FIXTURES).join(&name)).unwrap().len();
        let cap = if name == "hevc_4k.mp4" {
            MAX_HEVC_BYTES
        } else {
            MAX_FIXTURE_BYTES
        };
        assert!(
            size > 0 && size <= cap,
            "{name} is {size} bytes, over {cap}"
        );
    }
}

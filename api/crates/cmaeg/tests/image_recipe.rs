//! Contracts for the image build recipe; deployment policy is maintained privately.

const DOCKERFILE: &str = include_str!("../Dockerfile");
const FFMPEG_DIR: &str = "/opt/ffmpeg/bin";

/// The instruction lines of the Dockerfile stage `name`: from its `FROM ...
/// AS name` up to the next `FROM`, comments and blank lines left out.
fn stage<'a>(dockerfile: &'a str, name: &str) -> Vec<&'a str> {
    let header = format!(" AS {name}");
    dockerfile
        .lines()
        .map(str::trim)
        .skip_while(|line| !(line.starts_with("FROM ") && line.ends_with(&header)))
        .enumerate()
        .take_while(|(index, line)| *index == 0 || !line.starts_with("FROM "))
        .map(|(_, line)| line)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

/// A `COPY --from=<from>` line of `lines` naming every source in `sources`
/// and ending at `destination`.
fn copies(lines: &[&str], from: &str, sources: &[&str], destination: &str) -> bool {
    lines.iter().any(|line| {
        let words: Vec<&str> = line.split_whitespace().collect();
        words.first() == Some(&"COPY")
            && words.get(1) == Some(&format!("--from={from}").as_str())
            && words.last() == Some(&destination)
            && sources.iter().all(|source| words.contains(source))
    })
}

#[test]
fn the_runtime_image_ships_the_self_tested_ffmpeg_in_the_documented_dir() {
    let runtime = stage(DOCKERFILE, "runtime");
    assert!(!runtime.is_empty(), "the Dockerfile has no runtime stage");
    let binaries = [
        &format!("{FFMPEG_DIR}/ffmpeg")[..],
        &format!("{FFMPEG_DIR}/ffprobe")[..],
    ];
    let into_dir = format!("{FFMPEG_DIR}/");
    assert!(copies(&runtime, "ffmpeg-build", &binaries, &into_dir));
    let moved = DOCKERFILE.replace("/ffprobe /opt/ffmpeg/bin/", "/ffprobe /usr/bin/");
    assert!(!copies(
        &stage(&moved, "runtime"),
        "ffmpeg-build",
        &binaries,
        &into_dir
    ));
    // BuildKit skips a stage nothing copies from, so this keeps the gate.
    assert!(runtime
        .iter()
        .any(|line| line.starts_with("COPY --from=media-self-test ")));
    let self_test = stage(DOCKERFILE, "media-self-test");
    assert!(copies(&self_test, "ffmpeg-build", &binaries, &into_dir));
    assert!(self_test
        .iter()
        .any(|line| line.starts_with(&format!("CMAEG_MEDIA_FFMPEG_DIR={FFMPEG_DIR} "))));
    assert!(self_test
        .iter()
        .any(|line| line.contains("--media-self-test /opt/cmaeg-media-fixtures")));
    assert!(self_test.contains(&"RUN --network=none <<'MEDIA_SELF_TEST'"));
    // The stage copies exactly the fixtures the self-test's cases name.
    assert!(self_test
        .contains(&"COPY api/crates/cmaeg/tests/fixtures/media/ /opt/cmaeg-media-fixtures/"));
}

#[test]
fn the_ffmpeg_build_is_hash_pinned_and_lgpl_only() {
    let build = stage(DOCKERFILE, "ffmpeg-build");
    assert!(
        !build.is_empty(),
        "the Dockerfile has no ffmpeg-build stage"
    );
    assert!(build.iter().any(|line| line.contains("| sha256sum -c -")));
    assert!(build.contains(&"RUN --network=none <<'BUILD_FFMPEG'"));
    for forbidden in ["--enable-gpl", "--enable-nonfree", "--enable-version3"] {
        assert!(
            !build.iter().any(|line| line.contains(forbidden)),
            "{forbidden} in the ffmpeg-build stage"
        );
    }
    assert!(build.iter().any(|line| line.contains("--disable-network")));
    // The rule itself: a GPL switch anywhere in the stage is caught.
    let gpl = DOCKERFILE.replace("--disable-network", "--disable-network --enable-gpl");
    assert!(stage(&gpl, "ffmpeg-build")
        .iter()
        .any(|line| line.contains("--enable-gpl")));
    assert!(stage(DOCKERFILE, "no-such-stage").is_empty());
}

#[test]
fn local_preview_only_replaces_the_binary_and_preserves_the_standard_target() {
    let preview = stage(DOCKERFILE, "local-preview");
    assert!(preview[0].contains("${CMAEG_LOCAL_PREVIEW_BASE}"));
    assert!(copies(
        &preview,
        "local-binary",
        &["/cmaeg"],
        "/usr/local/bin/cmaeg"
    ));
    assert!(!preview.iter().any(|line| line.starts_with("RUN ")));
    assert!(preview
        .iter()
        .any(|line| line.contains("local-preview-no-transformer")));
    assert!(DOCKERFILE
        .lines()
        .rfind(|line| line.starts_with("FROM "))
        .unwrap()
        .ends_with(" AS runtime"));
}

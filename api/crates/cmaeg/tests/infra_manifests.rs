//! Rules that tie the shipped `infra/cmaeg` manifests to gateway configuration,
//! so a manifest change that enables a feature cannot merge without the
//! resources that feature needs.
//!
//! The manifests are hand-written YAML with two-space indentation, so a small
//! indentation-aware line scanner reads them; a YAML crate would be a new
//! dependency for a test that only needs a handful of scalar paths.

use cmaeg::media::transform::MEDIA_ROOT;

const CONFIGMAP: &str = include_str!("../../../../infra/cmaeg/configmap.yaml");
const DEPLOYMENT: &str = include_str!("../../../../infra/cmaeg/deployment.yaml");
const DOCKERFILE: &str = include_str!("../../../../infra/cmaeg/Dockerfile");
/// Where the image installs `ffmpeg` and `ffprobe`: the value
/// `CMAEG_MEDIA_FFMPEG_DIR` is documented to take.
const FFMPEG_DIR: &str = "/opt/ffmpeg/bin";
/// Media fetch and decode buffers need headroom a 1Gi limit does not leave, and
/// a request below the limit lets the scheduler overcommit the node.
const MIN_MEDIA_MEMORY_MIB: u64 = 1536;
/// The transformer adds one job (384 MiB of address space) and its 32 MiB
/// input volume to the media gate.
const MIN_TRANSFORMER_MEMORY_MIB: u64 = 2048;
const MIB: u64 = 1024 * 1024;

struct Line<'a> {
    indent: usize,
    text: &'a str,
}

/// Non-blank, non-comment lines with their indentation.
fn lines(yaml: &str) -> Vec<Line<'_>> {
    yaml.lines()
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
        .map(|line| Line {
            indent: line.len() - line.trim_start().len(),
            text: line.trim(),
        })
        .collect()
}

/// The scalar after `key:` on `line`, without surrounding quotes.
fn value<'a>(line: &Line<'a>, key: &str) -> Option<&'a str> {
    let rest = line.text.strip_prefix(key)?.strip_prefix(':')?;
    Some(rest.trim().trim_matches('"'))
}

/// The index of `key` among the direct children of the line at `parent`.
fn child(lines: &[Line<'_>], parent: usize, key: &str) -> Option<usize> {
    let block = lines.get(parent + 1..)?;
    let indent = block
        .first()
        .filter(|line| line.indent > lines[parent].indent)?
        .indent;
    block
        .iter()
        .take_while(|line| line.indent > lines[parent].indent)
        .position(|line| line.indent == indent && value(line, key).is_some())
        .map(|offset| parent + 1 + offset)
}

/// The value of `data.<key>` in a ConfigMap.
fn configmap_value<'a>(configmap: &'a str, key: &str) -> Option<&'a str> {
    let lines = lines(configmap);
    let data = lines
        .iter()
        .position(|line| line.indent == 0 && line.text == "data:")?;
    value(&lines[child(&lines, data, key)?], key)
}

/// The index of the list item `- name: <name>` inside the block at `parent`.
fn item(lines: &[Line<'_>], parent: usize, name: &str) -> Option<usize> {
    let item = format!("- name: {name}");
    lines[parent + 1..]
        .iter()
        .take_while(|line| line.indent > lines[parent].indent)
        .position(|line| line.text == item)
        .map(|offset| parent + 1 + offset)
}

/// The index of the container named `name`.
fn container(lines: &[Line<'_>], name: &str) -> Result<usize, String> {
    let containers = lines
        .iter()
        .position(|line| line.text == "containers:")
        .ok_or("the deployment has no containers")?;
    item(lines, containers, name).ok_or(format!("the deployment has no {name} container"))
}

/// `resources.<section>.memory` of the container named `name`, in bytes.
fn container_memory(deployment: &str, name: &str, section: &str) -> Result<u64, String> {
    let lines = lines(deployment);
    let container = container(&lines, name)?;
    let memory = child(&lines, container, "resources")
        .and_then(|resources| child(&lines, resources, section))
        .and_then(|section| child(&lines, section, "memory"))
        .ok_or(format!("{name} has no resources.{section}.memory"))?;
    let quantity = value(&lines[memory], "memory").unwrap_or_default();
    bytes(quantity).ok_or(format!(
        "{name} {section} memory {quantity:?} is not a quantity"
    ))
}

/// A Kubernetes memory quantity in bytes: plain digits or a binary or decimal suffix.
fn bytes(quantity: &str) -> Option<u64> {
    const SUFFIXES: [(&str, u64); 8] = [
        ("Ki", 1 << 10),
        ("Mi", 1 << 20),
        ("Gi", 1 << 30),
        ("Ti", 1 << 40),
        ("k", 1_000),
        ("M", 1_000_000),
        ("G", 1_000_000_000),
        ("T", 1_000_000_000_000),
    ];
    let (digits, scale) = SUFFIXES
        .iter()
        .find_map(|(suffix, scale)| Some((quantity.strip_suffix(suffix)?, *scale)))
        .unwrap_or((quantity, 1));
    digits.parse::<u64>().ok()?.checked_mul(scale)
}

/// With inbound media enabled, the gateway container's memory request must equal
/// its limit and both must be at least 1536Mi.
fn media_memory_rule(configmap: &str, deployment: &str) -> Result<(), String> {
    if configmap_value(configmap, "CMAEG_MEDIA_ENABLED") != Some("true") {
        return Ok(());
    }
    let request = container_memory(deployment, "cmaeg", "requests")?;
    let limit = container_memory(deployment, "cmaeg", "limits")?;
    if request != limit {
        return Err(format!(
            "CMAEG_MEDIA_ENABLED=true requires the cmaeg memory request ({request} bytes) to equal its limit ({limit} bytes)"
        ));
    }
    if limit < MIN_MEDIA_MEMORY_MIB * MIB {
        return Err(format!(
            "CMAEG_MEDIA_ENABLED=true requires at least {MIN_MEDIA_MEMORY_MIB}Mi of cmaeg memory, not {limit} bytes"
        ));
    }
    Ok(())
}

fn enabled(configmap: &str) -> String {
    let flipped = configmap.replace(
        "CMAEG_MEDIA_ENABLED: \"false\"",
        "CMAEG_MEDIA_ENABLED: \"true\"",
    );
    assert_ne!(flipped, configmap, "the shipped flag line changed shape");
    flipped
}

fn with_memory(deployment: &str, request: &str, limit: &str) -> String {
    let edited = deployment
        .replace("memory: 256Mi", &format!("memory: {request}"))
        .replace("memory: 1Gi", &format!("memory: {limit}"));
    assert_eq!(
        container_memory(&edited, "cmaeg", "requests"),
        Ok(bytes(request).unwrap())
    );
    assert_eq!(
        container_memory(&edited, "cmaeg", "limits"),
        Ok(bytes(limit).unwrap())
    );
    edited
}

/// Multi-consumer routing ships off: no routed event exists until an
/// operator flips the switch deliberately.
#[test]
fn the_multi_consumer_switch_ships_off() {
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_MULTI_CONSUMER_ENABLED"),
        Some("false")
    );
}

#[test]
fn media_enabled_requires_request_equal_limit_of_at_least_1536mi() {
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_MEDIA_ENABLED"),
        Some("false")
    );
    assert_eq!(media_memory_rule(CONFIGMAP, DEPLOYMENT), Ok(()));
    let on = enabled(CONFIGMAP);
    for (request, limit) in [("1536Mi", "1536Mi"), ("2Gi", "2Gi"), ("2Gi", "2048Mi")] {
        let deployment = with_memory(DEPLOYMENT, request, limit);
        assert_eq!(
            media_memory_rule(&on, &deployment),
            Ok(()),
            "{request}/{limit}"
        );
    }
    let unequal = with_memory(DEPLOYMENT, "1536Mi", "2Gi");
    assert!(media_memory_rule(&on, &unequal)
        .unwrap_err()
        .contains("to equal its limit"));
    // The flag off accepts any memory, which keeps the shipped default deployable.
    assert_eq!(media_memory_rule(CONFIGMAP, &unequal), Ok(()));
}

#[test]
fn media_enabled_with_1gi_is_rejected() {
    let on = enabled(CONFIGMAP);
    assert_eq!(container_memory(DEPLOYMENT, "cmaeg", "limits"), Ok(1 << 30));
    assert!(media_memory_rule(&on, DEPLOYMENT).is_err());
    let small = with_memory(DEPLOYMENT, "1Gi", "1Gi");
    assert!(media_memory_rule(&on, &small)
        .unwrap_err()
        .contains("at least 1536Mi"));
    let missing = DEPLOYMENT.replace("memory: 1Gi", "ephemeral-storage: 1Gi");
    assert_eq!(
        media_memory_rule(&on, &missing),
        Err("cmaeg has no resources.limits.memory".into())
    );
    let unreadable = DEPLOYMENT.replace("memory: 1Gi", "memory: lots");
    assert!(media_memory_rule(&on, &unreadable)
        .unwrap_err()
        .contains("is not a quantity"));
}

/// The media job volume as `(mountPath, medium, sizeLimit)`: where the
/// `cmaeg` container mounts it, and its `emptyDir` settings.
fn media_volume(deployment: &str) -> Result<(String, String, String), String> {
    let lines = lines(deployment);
    let container = container(&lines, "cmaeg")?;
    let mount = child(&lines, container, "volumeMounts")
        .and_then(|mounts| item(&lines, mounts, "media-jobs"))
        .and_then(|mount| child(&lines, mount, "mountPath"))
        .ok_or("the cmaeg container does not mount media-jobs")?;
    let volumes = lines
        .iter()
        .position(|line| line.text == "volumes:")
        .ok_or("the pod declares no volumes")?;
    let empty_dir = item(&lines, volumes, "media-jobs")
        .and_then(|volume| child(&lines, volume, "emptyDir"))
        .ok_or("media-jobs is not an emptyDir volume")?;
    let field = |key: &str| {
        child(&lines, empty_dir, key)
            .and_then(|line| value(&lines[line], key))
            .unwrap_or_default()
            .to_owned()
    };
    Ok((
        value(&lines[mount], "mountPath")
            .unwrap_or_default()
            .to_owned(),
        field("medium"),
        field("sizeLimit"),
    ))
}

/// With `CMAEG_MEDIA_FFMPEG_DIR` set, the gateway container needs a `sleep`
/// preStop (an exec'd process is dumpable and holds the Secret variables),
/// and guaranteed memory of at least 2Gi.
fn transformer_rule(configmap: &str, deployment: &str) -> Result<(), String> {
    if configmap_value(configmap, "CMAEG_MEDIA_FFMPEG_DIR").is_none_or(str::is_empty) {
        return Ok(());
    }
    let lines = lines(deployment);
    let container = container(&lines, "cmaeg")?;
    let pre_stop = child(&lines, container, "lifecycle")
        .and_then(|lifecycle| child(&lines, lifecycle, "preStop"))
        .ok_or("CMAEG_MEDIA_FFMPEG_DIR requires a sleep preStop hook on cmaeg")?;
    if child(&lines, pre_stop, "exec").is_some() || child(&lines, pre_stop, "sleep").is_none() {
        return Err(
            "CMAEG_MEDIA_FFMPEG_DIR requires the cmaeg preStop hook to be a sleep action, not exec"
                .into(),
        );
    }
    let request = container_memory(deployment, "cmaeg", "requests")?;
    let limit = container_memory(deployment, "cmaeg", "limits")?;
    if request != limit {
        return Err(format!(
            "CMAEG_MEDIA_FFMPEG_DIR requires the cmaeg memory request ({request} bytes) to equal its limit ({limit} bytes)"
        ));
    }
    if limit < MIN_TRANSFORMER_MEMORY_MIB * MIB {
        return Err(format!(
            "CMAEG_MEDIA_FFMPEG_DIR requires at least 2Gi of cmaeg memory, not {limit} bytes"
        ));
    }
    Ok(())
}

fn with_ffmpeg_dir(configmap: &str) -> String {
    let set = configmap.replace(
        "CMAEG_MEDIA_FFMPEG_DIR: \"\"",
        &format!("CMAEG_MEDIA_FFMPEG_DIR: \"{FFMPEG_DIR}\""),
    );
    assert_ne!(set, configmap, "the shipped directory line changed shape");
    set
}

fn with_sleep_pre_stop(deployment: &str) -> String {
    let edited = deployment.replace(
        "exec:\n                command: [\"/bin/sleep\", \"5\"]",
        "sleep:\n                seconds: 5",
    );
    assert_ne!(edited, deployment, "the shipped preStop hook changed shape");
    edited
}

#[test]
fn the_media_job_volume_is_a_32mi_memory_empty_dir_at_the_media_root() {
    assert_eq!(
        media_volume(DEPLOYMENT),
        Ok((MEDIA_ROOT.to_owned(), "Memory".into(), "32Mi".into()))
    );
    let moved = DEPLOYMENT.replace("mountPath: /var/cmaeg/media", "mountPath: /tmp/media");
    assert_ne!(media_volume(&moved).unwrap().0, MEDIA_ROOT);
    let unmounted = DEPLOYMENT.replace(
        "- name: media-jobs\n              mountPath",
        "- name: other\n              mountPath",
    );
    assert_eq!(
        media_volume(&unmounted),
        Err("the cmaeg container does not mount media-jobs".into())
    );
}

#[test]
fn the_ffmpeg_dir_requires_a_sleep_pre_stop_and_2gi_of_guaranteed_memory() {
    assert_eq!(
        configmap_value(CONFIGMAP, "CMAEG_MEDIA_FFMPEG_DIR"),
        Some("")
    );
    assert_eq!(transformer_rule(CONFIGMAP, DEPLOYMENT), Ok(()));
    let set = with_ffmpeg_dir(CONFIGMAP);
    let ready = with_sleep_pre_stop(&with_memory(DEPLOYMENT, "2Gi", "2Gi"));
    assert_eq!(transformer_rule(&set, &ready), Ok(()));
    let exec = with_memory(DEPLOYMENT, "2Gi", "2Gi");
    assert!(transformer_rule(&set, &exec)
        .unwrap_err()
        .contains("sleep action, not exec"));
    let small = with_sleep_pre_stop(&with_memory(DEPLOYMENT, "1536Mi", "1536Mi"));
    assert!(transformer_rule(&set, &small)
        .unwrap_err()
        .contains("at least 2Gi"));
    let burstable = with_sleep_pre_stop(&with_memory(DEPLOYMENT, "1536Mi", "2Gi"));
    assert!(transformer_rule(&set, &burstable)
        .unwrap_err()
        .contains("to equal its limit"));
    // The enablement also needs the media rule, which 2Gi satisfies.
    assert_eq!(media_memory_rule(&enabled(&set), &ready), Ok(()));
}

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

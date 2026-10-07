//! Rules that tie the shipped `infra/cmaeg` manifests to gateway configuration,
//! so a manifest change that enables a feature cannot merge without the
//! resources that feature needs.
//!
//! The manifests are hand-written YAML with two-space indentation, so a small
//! indentation-aware line scanner reads them; a YAML crate would be a new
//! dependency for a test that only needs a handful of scalar paths.

const CONFIGMAP: &str = include_str!("../../../../infra/cmaeg/configmap.yaml");
const DEPLOYMENT: &str = include_str!("../../../../infra/cmaeg/deployment.yaml");
/// Media fetch and decode buffers need headroom a 1Gi limit does not leave, and
/// a request below the limit lets the scheduler overcommit the node.
const MIN_MEDIA_MEMORY_MIB: u64 = 1536;
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

/// `resources.<section>.memory` of the container named `name`, in bytes.
fn container_memory(deployment: &str, name: &str, section: &str) -> Result<u64, String> {
    let lines = lines(deployment);
    let containers = lines
        .iter()
        .position(|line| line.text == "containers:")
        .ok_or("the deployment has no containers")?;
    let item = format!("- name: {name}");
    let container = lines[containers + 1..]
        .iter()
        .take_while(|line| line.indent > lines[containers].indent)
        .position(|line| line.text == item)
        .map(|offset| containers + 1 + offset)
        .ok_or(format!("the deployment has no {name} container"))?;
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

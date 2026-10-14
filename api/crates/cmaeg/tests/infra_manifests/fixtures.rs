//! Synthetic cases must not depend on the current production flag or resources.

use super::{bytes, child, container, container_memory, lines};

pub(super) fn set_config(configmap: &str, key: &str, value: &str) -> String {
    let prefix = format!("  {key}:");
    let mut matches = 0;
    let edited = configmap
        .lines()
        .map(|line| {
            if line.starts_with(&prefix) {
                matches += 1;
                format!("{prefix} \"{value}\"")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(matches, 1, "expected one ConfigMap key {key}");
    edited + "\n"
}

pub(super) fn with_memory(deployment: &str, request: &str, limit: &str) -> String {
    let mut edited = deployment.to_owned();
    for (section, quantity) in [("requests", request), ("limits", limit)] {
        let parsed = lines(&edited);
        let owner = container(&parsed, "cmaeg").unwrap();
        let resources = child(&parsed, owner, "resources").unwrap();
        let section = child(&parsed, resources, section).unwrap();
        let memory = child(&parsed, section, "memory").unwrap();
        // Locate the original line by its slice, so equal old request/limit
        // quantities and other containers cannot redirect the fixture mutation.
        let start = parsed[memory].text.as_ptr() as usize - edited.as_ptr() as usize;
        let end = start + parsed[memory].text.len();
        edited.replace_range(start..end, &format!("memory: {quantity}"));
    }
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

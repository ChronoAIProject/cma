//! Turn, tool, context, card and media code read the serving agent's
//! provider, profile and credential only through the consumer view, so that
//! adding consumers changes no caller. This test fails CI when a file outside
//! the allowlist reads those fields from the channel definition directly.
use std::path::{Path, PathBuf};

/// The direct reads the consumer view replaces.
const DIRECT_READS: &[&str] = &[
    "definition.provider",
    "provider_credential",
    "definition.profile",
];

/// Files under `src/` that may keep reading the stored fields, and why. An
/// entry ending in `/` covers its directory; one ending in `*` a file prefix.
const ALLOWED: &[(&str, &str)] = &[
    (
        "channels/management.rs",
        "storage: management reads and writes the stored binding",
    ),
    (
        "channels/lifecycle.rs",
        "storage: delete and rekey act on the stored binding",
    ),
    (
        "channels/create_cleanup_tests.rs",
        "storage fixtures: exercise incomplete records and retained credentials",
    ),
    (
        "channels/creation.rs",
        "storage: creation binds the stored provider",
    ),
    (
        "channels/definition*",
        "storage: definitions are built and patched here",
    ),
    (
        "channels/resolver.rs",
        "storage: restore validates the stored profile digest",
    ),
    (
        "channels/profile_revision.rs",
        "management: baseline and publish retained revisions on the stored binding",
    ),
    (
        "channels/profile_assignment_tests.rs",
        "storage fixtures: publish and retain synthetic worker revisions",
    ),
    (
        "connections/",
        "connection flows stay default-only by design",
    ),
    (
        "http/management.rs",
        "public projection: the creation response names its binding",
    ),
    (
        "http/management_responses.rs",
        "public projection of the stored binding",
    ),
    (
        "http/record_management.rs",
        "public owner inspection projects the retained binding independently of serving consumers",
    ),
    (
        "http/schema.rs",
        "public projection of the stored definition",
    ),
];

fn allowed(relative: &str) -> bool {
    ALLOWED
        .iter()
        .any(|(entry, _)| match entry.strip_suffix('*') {
            Some(prefix) => relative.starts_with(prefix),
            None if entry.ends_with('/') => relative.starts_with(entry),
            None => relative == *entry,
        })
}

/// Every `.rs` file under `root` that is outside the allowlist and mentions a
/// direct read, as `path: pattern` relative to `root`.
fn offenders(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect(root, &mut files);
    let mut found = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(root)
            .expect("collected under root")
            .to_string_lossy()
            .replace('\\', "/");
        if allowed(&relative) {
            continue;
        }
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|error| panic!("{} is unreadable: {error}", file.display()));
        for pattern in DIRECT_READS {
            if text.contains(pattern) {
                found.push(format!("{relative}: {pattern}"));
            }
        }
    }
    found.sort();
    found
}

fn collect(directory: &Path, files: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("{} is unreadable: {error}", directory.display()));
    for entry in entries {
        let path = entry.expect("directory entry is readable").path();
        if path.is_dir() {
            collect(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

fn source_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

#[test]
fn consumer_view_is_the_only_turn_path_reader() {
    let found = offenders(&source_root());
    assert!(
        found.is_empty(),
        "read these through ChannelDefinition::consumer or ResolvedChannel::consumer \
         instead:\n{}",
        found.join("\n")
    );
}

#[test]
fn every_allowlisted_path_exists() {
    let root = source_root();
    for (entry, _) in ALLOWED {
        let path = entry.trim_end_matches(['*', '/']);
        let exists = match entry.strip_suffix('*') {
            Some(prefix) => {
                let (directory, stem) = prefix.rsplit_once('/').unwrap_or(("", prefix));
                std::fs::read_dir(root.join(directory))
                    .unwrap()
                    .any(|file| {
                        file.unwrap()
                            .file_name()
                            .to_string_lossy()
                            .starts_with(stem)
                    })
            }
            None => root.join(path).exists(),
        };
        assert!(
            exists,
            "allowlisted {entry} no longer exists; remove the entry"
        );
    }
}

#[test]
fn a_direct_read_outside_the_allowlist_is_reported() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("consumer-readers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("turns")).unwrap();
    std::fs::create_dir_all(root.join("connections")).unwrap();
    std::fs::create_dir_all(root.join("http")).unwrap();
    let read = "let slug = &channel.definition.provider.slug;";
    std::fs::write(root.join("turns/runner.rs"), read).unwrap();
    std::fs::write(root.join("connections/service.rs"), read).unwrap();
    std::fs::write(root.join("http/record_management.rs"), read).unwrap();
    std::fs::write(root.join("http/record_management_extra.rs"), read).unwrap();
    std::fs::write(root.join("turns/notes.txt"), read).unwrap();
    let found = offenders(&root);
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(
        found,
        [
            "http/record_management_extra.rs: definition.provider",
            "turns/runner.rs: definition.provider"
        ]
    );
}

use std::path::{Component, Path, PathBuf};

use crate::failure::{Result, RuntimeError};

pub(crate) fn safe_component(value: &str, field: &'static str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || value == "."
        || value == ".."
        || value.starts_with('.')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(RuntimeError::rejected(
            "invalid_name",
            format!("{field} must be one safe path component"),
        ));
    }
    Ok(())
}

pub(crate) fn safe_relative(value: &str, field: &'static str) -> Result<PathBuf> {
    if value.is_empty() || value.len() > 1_024 || value.contains('\0') || value.contains('\\') {
        return Err(invalid_path(field));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || path.to_string_lossy() != value
    {
        return Err(invalid_path(field));
    }
    Ok(path.to_path_buf())
}

pub(crate) fn validate_selector(value: &str, field: &'static str) -> Result<()> {
    if value.is_empty()
        || value.len() > 255
        || value.starts_with('-')
        || value.contains('\0')
        || value.chars().any(char::is_whitespace)
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(RuntimeError::rejected(
            "invalid_git_selector",
            format!("{field} is not a safe Git revision"),
        ));
    }
    Ok(())
}

pub(crate) fn path_is_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}

pub(crate) fn lexical_target(root: &Path, link: &Path, target: &Path) -> Option<PathBuf> {
    let joined = if target.is_absolute() {
        target.to_path_buf()
    } else {
        link.parent()?.join(target)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            Component::CurDir => {}
        }
    }
    normalized.starts_with(root).then_some(normalized)
}

fn invalid_path(field: &'static str) -> RuntimeError {
    RuntimeError::rejected(
        "invalid_path",
        format!("{field} must be a normalized relative path"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_and_lexical_links_cannot_escape() {
        assert!(safe_relative("skills/a/SKILL.md", "path").is_ok());
        assert!(safe_relative("../outside", "path").is_err());
        let root = Path::new("/home/cma");
        let link = root.join("agent/home/auth.json");
        assert_eq!(
            lexical_target(root, &link, Path::new("../../provider/auth.json")),
            Some(root.join("provider/auth.json"))
        );
        assert!(lexical_target(root, &link, Path::new("../../../etc/passwd")).is_none());
    }
}

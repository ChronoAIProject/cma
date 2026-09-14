use std::collections::BTreeSet;

use super::{
    ProfileAuthoringService, ProfileAuthoringSkillPage, ProfileAuthoringSkillSearch,
    ResolveRepositoryRequest, ResolvedRepository,
};

pub(super) fn cursor(value: &str) -> bool {
    !value.is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control)
}

pub(super) fn query(value: &ProfileAuthoringSkillSearch) -> bool {
    value
        .q
        .as_ref()
        .is_none_or(|q| q.len() <= 200 && !q.chars().any(char::is_control))
        && value.limit.is_none_or(|limit| (1..=50).contains(&limit))
        && value.cursor.as_deref().is_none_or(cursor)
}

fn text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn description(value: Option<&str>) -> bool {
    value.is_none_or(|value| value.len() <= 6144 && !value.contains('\0'))
}

pub(super) fn services(values: &[ProfileAuthoringService]) -> bool {
    let mut seen = BTreeSet::new();
    values.len() <= 1000
        && values.iter().all(|entry| {
            text(&entry.id, 128)
                && entry.catalog_id.as_deref().is_none_or(|id| text(id, 128))
                && seen.insert(&entry.id)
                && text(&entry.name, 256)
                && description(entry.description.as_deref())
        })
}

fn version(value: &str) -> Option<(&str, &str)> {
    let (major, minor) = value.split_once('.')?;
    let decimal = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|byte| byte.is_ascii_digit())
            && (part == "0" || !part.starts_with('0'))
    };
    (value.len() <= 20 && decimal(major) && decimal(minor)).then_some((major, minor))
}

pub(super) fn skills(
    page: &ProfileAuthoringSkillPage,
    query: &ProfileAuthoringSkillSearch,
) -> bool {
    let mut seen = BTreeSet::new();
    page.items.len() <= usize::from(query.limit.unwrap_or(20))
        && page.next.as_deref().is_none_or(cursor)
        && (page.next.is_none() || page.next != query.cursor)
        && page.items.iter().all(|entry| {
            !entry.id.is_empty()
                && entry.id.len() <= 64
                && entry
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                && seen.insert(&entry.id)
                && text(&entry.name, 256)
                && description(entry.description.as_deref())
                && (1..=100).contains(&entry.versions.len())
                && entry.versions.iter().all(|value| version(value).is_some())
                && entry.versions.windows(2).all(|pair| {
                    let left = version(&pair[0]).unwrap();
                    let right = version(&pair[1]).unwrap();
                    (left.0.len(), left.0, left.1.len(), left.1)
                        > (right.0.len(), right.0, right.1.len(), right.1)
                })
        })
}

fn repository_identity(value: &str) -> Option<(String, String)> {
    let path = value.strip_prefix("https://github.com/")?;
    let path = path.strip_suffix('/').unwrap_or(path);
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let raw_repository = parts.next()?;
    let repository = raw_repository
        .strip_suffix(".git")
        .unwrap_or(raw_repository);
    let component = |part: &str| {
        !part.is_empty()
            && part.len() <= 100
            && !matches!(part, "." | "..")
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    (component(owner) && component(repository) && parts.next().is_none())
        .then(|| (owner.to_ascii_lowercase(), repository.to_ascii_lowercase()))
}

pub(super) fn repository(request: &ResolveRepositoryRequest, result: &ResolvedRepository) -> bool {
    let identity = repository_identity(&result.url);
    identity.is_some()
        && identity == repository_identity(&request.url)
        && result.reference == request.reference
        && !result.default_branch.is_empty()
        && result.default_branch.len() <= 255
        && !result.default_branch.starts_with('-')
        && !result.default_branch.chars().any(char::is_whitespace)
        && !result
            .default_branch
            .bytes()
            .any(|byte| byte.is_ascii_control())
        && result.commit_sha.len() == 40
        && result
            .commit_sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

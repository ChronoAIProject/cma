use std::collections::BTreeSet;

use super::{
    AuthoringServiceEntry, AuthoringSkillEntry, ProfilePage, RepositoryRequest,
    RepositoryResolution, SkillCatalogQuery, WorkspaceLocation, MAX_SKILL_PAGE_SIZE,
    MAX_SKILL_VERSIONS,
};

pub(super) fn query(query: &SkillCatalogQuery) -> bool {
    query.query.len() <= 200
        && !query.query.chars().any(char::is_control)
        && (1..=MAX_SKILL_PAGE_SIZE).contains(&query.limit)
        && query.cursor.as_deref().is_none_or(cursor)
}

fn cursor(value: &str) -> bool {
    !value.is_empty() && value.len() <= 2048 && !value.chars().any(char::is_control)
}

pub(super) fn services(entries: &[AuthoringServiceEntry]) -> bool {
    let mut seen = BTreeSet::new();
    entries.len() <= 1000
        && entries.iter().all(|entry| {
            text(entry.id.as_str(), 128)
                && entry.catalog_id.as_deref().is_none_or(|id| text(id, 128))
                && seen.insert(&entry.id)
                && text(&entry.name, 256)
                && description(entry.description.as_deref())
        })
}

pub(super) fn skills(page: &ProfilePage<AuthoringSkillEntry>, query: &SkillCatalogQuery) -> bool {
    let mut seen = BTreeSet::new();
    page.items.len() <= usize::from(query.limit)
        && page.next.as_deref().is_none_or(cursor)
        && (page.next.is_none() || page.next != query.cursor)
        && page.items.iter().all(|entry| {
            let id = entry.id.as_str();
            !id.is_empty()
                && id.len() <= 64
                && id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                && seen.insert(&entry.id)
                && text(&entry.name, 256)
                && description(entry.description.as_deref())
                && !entry.versions.is_empty()
                && entry.versions.len() <= MAX_SKILL_VERSIONS
                && entry.versions.iter().all(|value| version(value).is_some())
                && entry.versions.windows(2).all(|pair| {
                    let left = version(&pair[0]).unwrap();
                    let right = version(&pair[1]).unwrap();
                    (left.0.len(), left.0, left.1.len(), left.1)
                        > (right.0.len(), right.0, right.1.len(), right.1)
                })
        })
}

fn text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.len() <= max
        && !value.chars().any(char::is_control)
}

fn description(value: Option<&str>) -> bool {
    value.is_none_or(|value| value.len() <= 6144 && !value.contains('\0'))
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

pub(super) fn repository_identity(value: &str) -> Option<(String, String)> {
    let path = value.strip_prefix("https://github.com/")?;
    let path = path.strip_suffix('/').unwrap_or(path);
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let repository = parts.next()?;
    let repository = repository.strip_suffix(".git").unwrap_or(repository);
    let component = |value: &str| {
        !value.is_empty()
            && value.len() <= 100
            && !matches!(value, "." | "..")
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    (component(owner) && component(repository) && parts.next().is_none())
        .then(|| (owner.to_ascii_lowercase(), repository.to_ascii_lowercase()))
}

pub(super) fn repository(request: &RepositoryRequest, resolved: &RepositoryResolution) -> bool {
    WorkspaceLocation::GitRepository {
        url: resolved.url.clone(),
        reference: Some(resolved.default_branch.clone()),
    }
    .validate()
    .is_ok()
        && resolved.url.len() <= 2048
        && repository_identity(&request.url) == repository_identity(&resolved.url)
        && resolved.reference == request.reference
        && resolved.commit_sha.len() == 40
        && resolved
            .commit_sha
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

use axum::http::Uri;

use crate::failure::{Result, RuntimeError};

#[derive(Clone)]
pub(crate) struct TargetBase {
    url: url::Url,
    base_path: String,
}

impl TargetBase {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        let url = url::Url::parse(value).map_err(|_| invalid())?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || unsafe_path(url.path())
        {
            return Err(invalid());
        }
        let base_path = url.path().trim_end_matches('/').to_string();
        Ok(Self { url, base_path })
    }

    pub(crate) fn join(&self, uri: &Uri) -> Result<url::Url> {
        if unsafe_path(uri.path()) {
            return Err(RuntimeError::rejected(
                "proxy_path_invalid",
                "request path is outside the configured upstream",
            ));
        }
        let mut target = self.url.clone();
        let joined = format!("{}/{}", self.base_path, uri.path().trim_start_matches('/'));
        target.set_path(&joined);
        target.set_query(uri.query());
        let prefix = format!("{}/", self.base_path);
        if target.path() != self.base_path && !target.path().starts_with(&prefix) {
            return Err(RuntimeError::rejected(
                "proxy_path_invalid",
                "request path is outside the configured upstream",
            ));
        }
        Ok(target)
    }

    pub(crate) fn join_github(
        &self,
        uri: &Uri,
        route: &layer3_adapter::GithubCredentialRoute,
    ) -> Result<url::Url> {
        let prefix = self
            .base_path
            .strip_suffix("/api-github")
            .ok_or_else(invalid)?;
        let selected = Self {
            url: self.url.clone(),
            base_path: format!("{prefix}/{}", route.slug),
        };
        let mut target = selected.join(uri)?;
        if target.query_pairs().any(|(key, _)| key == "_nyxid_via") {
            return Err(RuntimeError::rejected(
                "proxy_github_selector_supplied",
                "GitHub connection selection is managed by the Sandbox",
            ));
        }
        target
            .query_pairs_mut()
            .append_pair("_nyxid_via", &route.service_id);
        Ok(target)
    }
}

pub(crate) fn unsafe_path(path: &str) -> bool {
    if !path.starts_with('/') || path.as_bytes().contains(&b'\\') {
        return true;
    }
    path.split('/').any(unsafe_segment)
}

fn unsafe_segment(segment: &str) -> bool {
    let mut decoded = segment.as_bytes().to_vec();
    for _ in 0..=segment.len() {
        if decoded == b"." || decoded == b".." {
            return true;
        }
        if decoded
            .iter()
            .any(|byte| matches!(byte, b'/' | b'\\' | 0..=31 | 127))
        {
            return true;
        }
        if !decoded.contains(&b'%') {
            return false;
        }
        let Some(next) = percent_decode(&decoded) else {
            return true;
        };
        if next == decoded {
            return false;
        }
        decoded = next;
    }
    true
}

fn percent_decode(value: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        if value[index] != b'%' {
            decoded.push(value[index]);
            index += 1;
            continue;
        }
        let high = hex(*value.get(index + 1)?)?;
        let low = hex(*value.get(index + 2)?)?;
        decoded.push((high << 4) | low);
        index += 3;
    }
    Some(decoded)
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn invalid() -> RuntimeError {
    RuntimeError::rejected(
        "proxy_upstream_invalid",
        "configured proxy upstream is not a safe HTTP URL",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursive_path_navigation_is_refused() {
        let base = TargetBase::parse("https://example.test/api/v1/proxy/s/service").unwrap();
        assert!(base.join(&"/models?limit=1".parse().unwrap()).is_ok());
        for path in ["/../secret", "/%2e%2e/secret", "/%252e%252e/secret"] {
            assert!(
                base.join(&path.parse().unwrap()).is_err(),
                "accepted {path}"
            );
        }
    }
}

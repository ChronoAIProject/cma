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
            .strip_suffix("/s/api-github")
            .ok_or_else(invalid)?;
        self.join_selected(
            uri,
            format!("{prefix}/{}", route.catalog_id),
            &route.service_id,
        )
    }

    pub(crate) fn join_selected_platform(
        &self,
        uri: &Uri,
        route: &layer3_adapter::GithubCredentialRoute,
    ) -> Result<url::Url> {
        if unsafe_path(uri.path()) {
            return self.join(uri);
        }
        let path = uri.path().trim_start_matches('/');
        let proxy_path = path
            .strip_prefix("api/v1/proxy/")
            .or_else(|| path.strip_prefix("proxy/"));
        let Some(proxy_path) = proxy_path else {
            if path.contains('%') {
                return Err(unproved_service());
            }
            return self.join(uri);
        };
        let (named, path) = match proxy_path.strip_prefix("s/") {
            Some(path) => (true, path),
            None => (false, proxy_path),
        };
        let (selector, suffix) = path.split_once('/').unwrap_or((path, ""));
        let (prefix, service_id) = if (named && selector == "api-github")
            || (!named
                && [route.catalog_id.as_str(), route.service_id.as_str()].contains(&selector))
        {
            (
                format!("/api/v1/proxy/{}", route.catalog_id),
                route.service_id.as_str(),
            )
        } else {
            let mut services = route.other_services.iter().filter(|service| {
                if named {
                    service.slug == selector
                } else {
                    service.service_id == selector
                        || service.catalog_id.as_deref() == Some(selector)
                }
            });
            let service = services.next().ok_or_else(unproved_service)?;
            if services.next().is_some() {
                return Err(unproved_service());
            }
            let prefix = match &service.catalog_id {
                Some(catalog) => format!("/api/v1/proxy/{catalog}"),
                None => format!("/api/v1/proxy/s/{}", service.slug),
            };
            (prefix, service.service_id.as_str())
        };
        let mut endpoint = format!("/{suffix}");
        if let Some(query) = uri.query() {
            endpoint.push('?');
            endpoint.push_str(query);
        }
        self.join_selected(
            &endpoint.parse().map_err(|_| unproved_service())?,
            format!("{}{prefix}", self.base_path),
            service_id,
        )
    }

    fn join_selected(&self, uri: &Uri, base_path: String, service_id: &str) -> Result<url::Url> {
        let mut target = Self {
            url: self.url.clone(),
            base_path,
        }
        .join(uri)?;
        if target.query_pairs().any(|(key, _)| key == "_nyxid_via") {
            return Err(RuntimeError::rejected(
                "proxy_github_selector_supplied",
                "service connection selection is managed by the Sandbox",
            ));
        }
        target
            .query_pairs_mut()
            .append_pair("_nyxid_via", service_id);
        Ok(target)
    }
}

fn unproved_service() -> RuntimeError {
    RuntimeError::rejected(
        "proxy_service_route_unproved",
        "the scoped credential has no proved route for this platform service",
    )
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

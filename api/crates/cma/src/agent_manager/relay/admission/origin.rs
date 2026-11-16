//! Original request proof is ephemeral. It never becomes a trusted wire context.
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Uri};
use layer2_core::sandbox::agent_io::OwnerCommandRoutingEvidence;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const MAX_EVIDENCE_BYTES: usize = 128 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::agent_manager::relay) struct Origin {
    method: String,
    uri: String,
    headers: Vec<(String, Vec<u8>)>,
}
impl Origin {
    pub(in crate::agent_manager::relay) fn capture(
        method: &Method,
        uri: &Uri,
        headers: &HeaderMap,
    ) -> Result<Self, ()> {
        let value = Self {
            method: method.to_string(),
            uri: uri.path_and_query().ok_or(())?.to_string(),
            headers: headers
                .iter()
                .filter(|(name, _)| super::super::authentication::forwarded(name.as_str()))
                .map(|(name, value)| (name.to_string(), value.as_bytes().to_vec()))
                .collect(),
        };
        value.parts()?;
        Ok(value)
    }
    pub(in crate::agent_manager::relay) fn evidence(
        &self,
    ) -> Result<OwnerCommandRoutingEvidence, ()> {
        let bytes = serde_json::to_vec(self).map_err(|_| ())?;
        if bytes.len() > MAX_EVIDENCE_BYTES {
            return Err(());
        }
        Ok(OwnerCommandRoutingEvidence::new(Arc::from(bytes)))
    }
    pub(super) fn current() -> Result<Option<Self>, ()> {
        OwnerCommandRoutingEvidence::current()
            .map(|value| {
                if value.bytes().len() > MAX_EVIDENCE_BYTES {
                    return Err(());
                }
                serde_json::from_slice(value.bytes()).map_err(|_| ())
            })
            .transpose()
    }
    pub(super) fn parts(&self) -> Result<(Method, Uri, HeaderMap), ()> {
        let method = Method::from_bytes(self.method.as_bytes()).map_err(|_| ())?;
        let uri: Uri = self.uri.parse().map_err(|_| ())?;
        if !self.uri.starts_with('/')
            || self.uri.starts_with(super::super::PREFIX)
            || uri.authority().is_some()
            || uri.scheme().is_some()
            || self.headers.len() > 256
        {
            return Err(());
        }
        let mut headers = HeaderMap::new();
        let mut size = self.uri.len();
        for (name, value) in &self.headers {
            size = size.checked_add(name.len() + value.len()).ok_or(())?;
            if size > MAX_EVIDENCE_BYTES || !super::super::authentication::forwarded(name) {
                return Err(());
            }
            headers.append(
                HeaderName::from_bytes(name.as_bytes()).map_err(|_| ())?,
                HeaderValue::from_bytes(value).map_err(|_| ())?,
            );
        }
        Ok((method, uri, headers))
    }
}

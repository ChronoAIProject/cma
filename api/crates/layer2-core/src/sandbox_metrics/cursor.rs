//! Authenticated cache cursors never confer authority themselves.
use super::{MetricsError, Order, Result, Scope, Sort};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use ring::hmac;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cursor {
    pub principal: String,
    pub admin: bool,
    pub filter_hash: String,
    pub report_id: String,
    pub sort: Sort,
    pub order: Order,
    pub last_value: serde_json::Value,
    pub last_null: bool,
    pub sandbox_id: String,
}
pub struct CursorSigner(hmac::Key);
impl CursorSigner {
    pub fn new(key: &[u8]) -> Self {
        Self(hmac::Key::new(hmac::HMAC_SHA256, key))
    }
    pub fn encode(&self, value: &Cursor) -> Result<String> {
        let body = serde_json::to_vec(value).map_err(|_| invalid())?;
        let tag = hmac::sign(&self.0, &body);
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(body),
            URL_SAFE_NO_PAD.encode(tag.as_ref())
        ))
    }
    pub fn decode(
        &self,
        value: &str,
        scope: &Scope,
        report: &str,
        hash: &str,
        sort: Sort,
        order: Order,
    ) -> Result<Cursor> {
        if value.len() > 4096 {
            return Err(invalid());
        }
        let (body, tag) = value.split_once('.').ok_or_else(invalid)?;
        let body = URL_SAFE_NO_PAD.decode(body).map_err(|_| invalid())?;
        let tag = URL_SAFE_NO_PAD.decode(tag).map_err(|_| invalid())?;
        hmac::verify(&self.0, &body, &tag).map_err(|_| invalid())?;
        let value: Cursor = serde_json::from_slice(&body).map_err(|_| invalid())?;
        if value.principal != scope.principal
            || value.admin != scope.admin
            || value.report_id != report
            || value.filter_hash != hash
            || value.sort != sort
            || value.order != order
        {
            return Err(invalid());
        }
        Ok(value)
    }
}
fn invalid() -> MetricsError {
    MetricsError::Invalid("Cursor does not match this principal, report or sort.".into())
}

use std::time::{Duration, SystemTime};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::Deserialize;
use zeroize::Zeroizing;

use crate::auth::persistence::SecretValue;

use super::SubscriptionError;

#[path = "subscription_tokens_ingress.rs"]
mod ingress;

const MAX_CACHE_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheDecodeError {
    Refused(layer3_adapter::ingress_budget::AdmissionRefusal),
    Account(SubscriptionError),
}

/// Native Codex owns the OAuth protocol. Only its private, encrypted cache enters
/// this boundary; neither browser-submitted JWTs nor sandbox auth files do.
#[derive(Debug, Clone)]
pub(crate) struct SubscriptionTokens {
    pub access_token: SecretValue,
    pub account_id: String,
    pub plan_type: Option<String>,
    pub expires_at: SystemTime,
}

#[derive(Deserialize)]
struct NativeAuth {
    tokens: NativeTokens,
}

#[derive(Deserialize)]
struct NativeTokens {
    access_token: String,
    refresh_token: String,
    id_token: String,
    account_id: String,
}

impl Drop for NativeTokens {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.id_token.zeroize();
    }
}

#[derive(Deserialize)]
struct Claims {
    exp: u64,
    #[serde(rename = "https://api.openai.com/auth")]
    auth: AccountClaims,
}

#[derive(Deserialize)]
struct AccountClaims {
    chatgpt_account_id: String,
    chatgpt_plan_type: Option<String>,
}

impl SubscriptionTokens {
    pub(crate) fn from_native_cache(cache: &SecretValue) -> Result<Self, SubscriptionError> {
        if cache.expose().len() > MAX_CACHE_BYTES {
            return Err(SubscriptionError::InvalidAccount);
        }
        let native: NativeAuth =
            serde_json::from_str(cache.expose()).map_err(|_| SubscriptionError::InvalidAccount)?;
        let token = &native.tokens;
        if token.refresh_token.trim().is_empty()
            || token.id_token.trim().is_empty()
            || token.account_id.is_empty()
            || token.account_id.len() > 256
            || token.account_id.chars().any(char::is_control)
        {
            return Err(SubscriptionError::InvalidAccount);
        }
        let mut segments = token.access_token.split('.');
        let _header = segments.next();
        let payload = segments.next().ok_or(SubscriptionError::InvalidAccount)?;
        if segments.next().is_none() || segments.next().is_some() {
            return Err(SubscriptionError::InvalidAccount);
        }
        let decoded = Zeroizing::new(
            URL_SAFE_NO_PAD
                .decode(payload)
                .map_err(|_| SubscriptionError::InvalidAccount)?,
        );
        let claims: Claims =
            serde_json::from_slice(&decoded).map_err(|_| SubscriptionError::InvalidAccount)?;
        if claims.auth.chatgpt_account_id != token.account_id {
            return Err(SubscriptionError::InvalidAccount);
        }
        let expires_at = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(claims.exp))
            .ok_or(SubscriptionError::InvalidAccount)?;
        Ok(Self {
            access_token: SecretValue::new(token.access_token.clone()),
            account_id: token.account_id.clone(),
            plan_type: claims.auth.chatgpt_plan_type,
            expires_at,
        })
    }

    pub(crate) fn usable_at(&self, now: SystemTime) -> bool {
        now.checked_add(Duration::from_secs(120))
            .is_some_and(|safe_until| self.expires_at > safe_until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn cache(account: &str, claim_account: &str, exp: u64) -> SecretValue {
        let claims = serde_json::json!({
            "exp": exp,
            "https://api.openai.com/auth": {
                "chatgpt_account_id": claim_account, "chatgpt_plan_type": "plus"
            }
        });
        let access = format!(
            "fixture.{}.fixture",
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        SecretValue::new(
            serde_json::json!({"tokens": {
                "access_token": access, "refresh_token": "fixture-refresh-secret",
                "id_token": "fixture-id-secret", "account_id": account
            }})
            .to_string(),
        )
    }

    #[test]
    fn expiry_margin_and_debug_redaction() {
        let tokens = SubscriptionTokens::from_native_cache(&cache("one", "one", 1000)).unwrap();
        assert!(tokens.usable_at(SystemTime::UNIX_EPOCH + Duration::from_secs(879)));
        assert!(!tokens.usable_at(SystemTime::UNIX_EPOCH + Duration::from_secs(880)));
        assert_eq!(tokens.account_id, "one");
        assert_eq!(tokens.plan_type.as_deref(), Some("plus"));
        assert!(!format!("{tokens:?}").contains(tokens.access_token.expose()));
    }

    #[test]
    fn rejects_cross_account_and_nonrenewable_caches() {
        assert_eq!(
            SubscriptionTokens::from_native_cache(&cache("one", "two", 1000)).unwrap_err(),
            SubscriptionError::InvalidAccount
        );
        let missing = SecretValue::new(
            cache("one", "one", 1000)
                .expose()
                .replace("fixture-refresh-secret", ""),
        );
        assert_eq!(
            SubscriptionTokens::from_native_cache(&missing).unwrap_err(),
            SubscriptionError::InvalidAccount
        );
        assert_eq!(
            SubscriptionTokens::from_native_cache(&SecretValue::new("{}".into())).unwrap_err(),
            SubscriptionError::InvalidAccount
        );
    }
}

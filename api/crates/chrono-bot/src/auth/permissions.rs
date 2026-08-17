//! What a caller is entitled to, and where that answer comes from.
//!
//! Split from the flow so that file is about the browser dance and this one
//! about the claim it ends with. Two sources, in order of preference: the token
//! itself, and — when it carries none — the provider's introspection endpoint.
//!
//! Worth reading on its own because it is the input to every authorization
//! decision this service makes. A permission read wrongly here is not a failed
//! request; it is a caller quietly holding more than they were granted.

use super::{AuthMode, AuthState, ProviderMetadata};

/// The permissions a userinfo response carries, if it carries any.
pub(super) fn permissions_claim(body: &serde_json::Value) -> Vec<String> {
    body.get("permissions")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// What the provider says this token may do, asked as this service.
///
/// `None` when the provider advertises no introspection endpoint, which is the
/// signal to fall back to the userinfo claim rather than to deny.
///
/// The credentials travel in the form body rather than as basic auth: both are
/// advertised, and this is the one the provider actually accepts here. An
/// inactive token yields no permissions rather than an error — it is an answer,
/// not a failure, and the subject it was resolved for is refused by the same
/// deny-by-default path as any caller holding nothing.
pub(super) async fn introspect_permissions(
    auth: &AuthState,
    meta: &ProviderMetadata,
    access_token: &str,
) -> Result<Option<Vec<String>>, String> {
    let Some(endpoint) = meta.introspection_endpoint.as_ref() else {
        return Ok(None);
    };
    let AuthMode::Provider {
        client_id,
        client_secret,
        ..
    } = &auth.mode
    else {
        return Ok(None);
    };

    let form = [
        ("token", access_token),
        ("token_type_hint", "access_token"),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
    ];
    let resp = auth
        .http
        .post(endpoint)
        .form(&form)
        // The form carries the secret, so nothing here logs it.
        .send()
        .await
        .map_err(|e| format!("introspection request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("introspection returned {}", resp.status()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("introspection body not usable: {e}"))?;
    if !body
        .get("active")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Ok(Some(Vec::new()));
    }
    Ok(Some(permissions_claim(&body)))
}

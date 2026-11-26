//! Strict extraction of one HTTP bearer credential.

use axum::http::{header, HeaderMap};
use base64::Engine as _;

/// A negative filter, never evidence of authentication. NyxID relay tokens
/// resolve to their owner's subject and RBAC at userinfo, but their authority
/// is restricted to bridge ingress/reply. Ordinary CMA authentication must not
/// turn that subject projection into a dashboard or owner-operation credential.
/// Every token that passes this filter still requires provider verification.
pub(super) fn permits_ambient_auth(token: &str) -> bool {
    if token.len() > 64 * 1024 {
        return false;
    }
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(_), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return true;
    };
    if base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(header)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(header))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|value| {
            value.get("typ").and_then(serde_json::Value::as_str)
                == Some("nyxid-resource-request+jwt")
        })
    {
        return false;
    }
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(payload))
    else {
        return true;
    };
    let Ok(serde_json::Value::Object(claims)) = serde_json::from_slice::<serde_json::Value>(&bytes)
    else {
        return true;
    };
    if claims
        .get("relay")
        .is_some_and(|value| !value.is_null() && value != &serde_json::Value::Bool(false))
        || claims
            .get("token_type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|kind| {
                matches!(kind, "relay_reply" | "relay_callback" | "resource_request")
            })
        || claims
            .get("nyx_request")
            .is_some_and(|value| !value.is_null())
    {
        return false;
    }
    ![
        "relay_api_key_id",
        "relay_api_key_name",
        "relay_allowed_service_ids",
        "relay_allowed_node_ids",
        "relay_allow_all_services",
        "relay_allow_all_nodes",
    ]
    .iter()
    .any(|field| claims.get(*field).is_some_and(|value| !value.is_null()))
}

/// Negative restriction only. The provider still verifies the complete token.
/// Service-account and delegated actor tokens cannot acquire ambient org rights.
pub(super) fn permits_personal_authority(token: &str) -> bool {
    if !permits_ambient_auth(token) {
        return false;
    }
    let pieces: Vec<_> = token.split('.').collect();
    if pieces.len() != 3 {
        return true;
    }
    let decode = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(pieces[1])
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(pieces[1]));
    let Ok(bytes) = decode else {
        return false;
    };
    let Ok(body) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    if !body.is_object() {
        return false;
    }
    for key in ["sa", "delegated"] {
        if body
            .get(key)
            .is_some_and(|value| !value.is_null() && value.as_bool() != Some(false))
        {
            return false;
        }
    }
    body.get("act").is_none_or(serde_json::Value::is_null)
}

/// Negative restriction for independently retained API authority. NyxID agent
/// keys require separate service-bound introspection. Other opaque tokens cannot
/// prove an ordinary access-token shape. JWT
/// content only restricts eligibility; provider introspection authenticates
/// the exact token, current permissions, subject and expiry separately.
pub(super) fn permits_independent_commands(token: &str) -> bool {
    if super::agent_keys::is_agent_key(token) {
        return token.len() <= 64 * 1024;
    }
    if !permits_personal_authority(token) {
        return false;
    }
    let pieces: Vec<_> = token.split('.').collect();
    if pieces.len() != 3 {
        return false;
    }
    let Ok(bytes) = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(pieces[1])
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(pieces[1]))
    else {
        return false;
    };
    let Ok(serde_json::Value::Object(claims)) = serde_json::from_slice(&bytes) else {
        return false;
    };
    claims.get("token_type").and_then(serde_json::Value::as_str) == Some("access")
        && ![
            "cnf",
            "resources",
            "resource",
            "allowed_service_ids",
            "allowed_node_ids",
            "allow_all_services",
            "allow_all_nodes",
            "api_key_id",
            "assistant_forward",
        ]
        .iter()
        .any(|field| claims.get(*field).is_some_and(|value| !value.is_null()))
}

pub fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer")
        || token.is_empty()
        || token != token.trim()
        || token
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return None;
    }
    Some(token.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_one_well_formed_bearer_is_accepted() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer token-a".parse().unwrap());
        assert_eq!(bearer_token(&headers).as_deref(), Some("token-a"));

        for invalid in [
            "Basic token-a",
            "Bearer",
            "Bearer  token-a",
            "Bearer token a",
        ] {
            headers.insert(header::AUTHORIZATION, invalid.parse().unwrap());
            assert!(bearer_token(&headers).is_none(), "accepted {invalid:?}");
        }
        headers.append(header::AUTHORIZATION, "Bearer token-b".parse().unwrap());
        assert!(bearer_token(&headers).is_none());
    }

    #[test]
    fn channel_tokens_are_refused_even_when_their_subject_has_owner_permissions() {
        for claims in [
            serde_json::json!({"sub":"owner", "permissions":["cma:*"], "relay":true}),
            serde_json::json!({"token_type":"relay_reply"}),
            serde_json::json!({"token_type":"relay_callback"}),
            serde_json::json!({"relay":false, "relay_api_key_id":"selected-key"}),
            serde_json::json!({"relay_allowed_service_ids":[]}),
            serde_json::json!({"relay_allow_all_services":false}),
            serde_json::json!({"relay":"true"}),
            serde_json::json!({"token_type":"resource_request"}),
            serde_json::json!({"nyx_request": {"version":1}}),
        ] {
            let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&claims).unwrap());
            assert!(!permits_ambient_auth(&format!(
                "header.{payload}.signature"
            )));
        }
    }

    #[test]
    fn request_proof_header_type_cannot_be_downgraded_to_an_ambient_bearer() {
        let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(
                &serde_json::json!({"alg":"RS256", "typ":"nyxid-resource-request+jwt"}),
            )
            .unwrap(),
        );
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({"sub":"owner", "permissions":["cma:*"]}))
                .unwrap(),
        );
        assert!(!permits_ambient_auth(&format!(
            "{header}.{payload}.signature"
        )));
    }

    #[test]
    fn ordinary_credentials_still_require_the_existing_provider_verifier() {
        for value in [
            "opaque",
            "unverified.payload.signature",
            "opaque.with.extra.parts",
        ] {
            assert!(permits_ambient_auth(value));
        }
        for claims in [
            serde_json::json!({"sub":"owner", "token_type":"access"}),
            serde_json::json!({"relay":false}),
            serde_json::json!({"relay":null}),
        ] {
            let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&claims).unwrap());
            assert!(permits_ambient_auth(&format!("header.{payload}.signature")));
        }
        assert!(!permits_ambient_auth(&"a".repeat(64 * 1024 + 1)));
    }
}

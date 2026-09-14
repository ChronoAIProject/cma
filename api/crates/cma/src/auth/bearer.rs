//! Strict extraction of one HTTP bearer credential.

use axum::http::{header, HeaderMap};

pub(super) fn bearer_token(headers: &HeaderMap) -> Option<String> {
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
}

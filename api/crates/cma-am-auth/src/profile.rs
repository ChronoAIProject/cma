//! Who the signed-in person is, as something worth showing them.
//!
//! The subject identifier is an opaque string. It is the right thing to key
//! state on and the wrong thing to put in front of a person, so the provider's
//! own account details are captured at sign-in and kept alongside it.
//!
//! Nothing here is authoritative. It is display material: if a field is absent
//! the interface falls back rather than inventing one, and no decision is ever
//! made on the strength of a display name.

use serde::{Deserialize, Serialize};

/// A signed-in person, as the interface should present them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// The provider's account name, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The account's email address, when the provider releases it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Absolute URL of the account's picture, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

impl Profile {
    /// Read a profile out of a userinfo response.
    ///
    /// Every field is optional and read defensively. A provider that omits one,
    /// or returns it empty, must leave the interface with a clear absence to
    /// fall back on rather than a blank string that renders as nothing.
    pub fn from_userinfo(body: &serde_json::Value) -> Self {
        // Preference order follows how specific each claim is about a person:
        // a full name first, then a chosen handle, then the given name.
        let display_name = first_non_empty(
            body,
            &["name", "display_name", "preferred_username", "given_name"],
        );
        let email = first_non_empty(body, &["email"]);
        let avatar_url = first_non_empty(body, &["picture", "avatar_url", "avatar"]);
        Self {
            display_name,
            email,
            avatar_url,
        }
    }

    /// Whether the provider told us anything worth showing.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.display_name.is_none() && self.email.is_none() && self.avatar_url.is_none()
    }
}

/// The first of these claims that carries a non-empty string.
///
/// Whitespace counts as empty: a provider returning " " has not given a name,
/// and treating it as one produces a label that looks like a rendering bug.
fn first_non_empty(body: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|k| body.get(*k))
        .filter_map(|v| v.as_str())
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_full_name_is_preferred_over_a_handle() {
        let p = Profile::from_userinfo(&json!({
            "name": "Shining Wang",
            "preferred_username": "shining",
            "email": "shining@example.com",
        }));
        assert_eq!(p.display_name.as_deref(), Some("Shining Wang"));
    }

    #[test]
    fn a_handle_is_used_when_there_is_no_full_name() {
        let p = Profile::from_userinfo(&json!({ "preferred_username": "shining" }));
        assert_eq!(p.display_name.as_deref(), Some("shining"));
    }

    #[test]
    fn a_blank_claim_counts_as_absent() {
        // Otherwise the interface renders an empty label and looks broken,
        // instead of falling back to something it can draw.
        let p = Profile::from_userinfo(&json!({ "name": "   ", "email": "" }));
        assert!(p.display_name.is_none());
        assert!(p.email.is_none());
    }

    #[test]
    fn an_avatar_is_read_from_any_of_the_usual_claims() {
        for key in ["picture", "avatar_url", "avatar"] {
            let p = Profile::from_userinfo(&json!({ key: "https://example/a.png" }));
            assert_eq!(
                p.avatar_url.as_deref(),
                Some("https://example/a.png"),
                "claim {key} should have been read"
            );
        }
    }

    #[test]
    fn a_provider_that_releases_nothing_leaves_an_empty_profile() {
        // The subject identifier still exists; this only says there is nothing
        // better to show, and the interface decides what to draw instead.
        let p = Profile::from_userinfo(&json!({ "sub": "76fe9d91" }));
        assert!(p.is_empty());
    }

    #[test]
    fn absent_fields_are_omitted_rather_than_sent_as_null() {
        // The client distinguishes "no name" from "name: null" only by their
        // absence, and omitting keeps the payload honest about what is known.
        let p = Profile::from_userinfo(&json!({ "email": "a@b.c" }));
        let rendered = serde_json::to_string(&p).expect("serialises");
        assert!(!rendered.contains("display_name"), "got {rendered}");
        assert!(rendered.contains("a@b.c"));
    }
}

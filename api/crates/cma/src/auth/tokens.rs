//! What the provider says, and what it issues.
//!
//! Split from the state so that file is about running a flow and this one about
//! the two things that come back from the other end: the document describing
//! where a provider's endpoints are, and the tokens it hands out for a subject.
//!
//! Worth its own file for the reason `SubjectTokens`' own `Debug` exists:
//! everything in half of it is a live credential, and the rules that keep one
//! out of a log are easier to keep when they are not surrounded by anything
//! else.

use serde::Deserialize;

/// What the provider issued for a signed-in subject.
///
/// The refresh token is the reason this is stored at all: an access token
/// outlives neither a long terminal session nor a long agent turn, and the only
/// alternative to renewing it is interrupting the user.
#[derive(Clone)]
#[cfg(test)]
pub struct SubjectTokens {
    /// Currently valid access token.
    pub access_token: String,
    /// Used to obtain the next access token, when the provider issued one.
    pub refresh_token: Option<String>,
    /// When the access token stops being accepted.
    pub expires_at: std::time::Instant,
}

#[cfg(test)]
impl std::fmt::Debug for SubjectTokens {
    /// Redacted: no `{:?}` may put a live token in a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubjectTokens")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[cfg(test)]
impl SubjectTokens {
    /// Whether the access token should be renewed now.
    ///
    /// Renewed early rather than on expiry: a token that dies mid-turn fails a
    /// request the user is waiting on, and the margin costs nothing.
    pub fn needs_refresh(&self, margin: std::time::Duration) -> bool {
        std::time::Instant::now() + margin >= self.expires_at
    }
}

/// Provider metadata, as far as this service uses it.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ProviderMetadata {
    pub(crate) authorization_endpoint: String,
    pub(crate) token_endpoint: String,
    #[serde(default)]
    pub(crate) userinfo_endpoint: Option<String>,
    /// Where a token's own grants can be asked about.
    ///
    /// Separate from userinfo because the two answer different questions. A
    /// provider releases roles and permissions through userinfo only when the
    /// token was granted a scope asking for them, and a token minted for
    /// another client — which is every token on the bearer path — was not.
    /// Introspection is asked by this service as itself, so it answers for any
    /// token the provider issued regardless of what that token was granted.
    #[serde(default)]
    pub(crate) introspection_endpoint: Option<String>,
}

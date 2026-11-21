//! The console's sign-in state, sealed into the `__Host-cmaeg-console-login`
//! cookie as a `cmaeg1.cl` token. Sealing keeps the gateway stateless across
//! the OAuth round trip, so a callback works on whichever pod receives it.
use super::{open, seal, MasterKeyRing, OpenError, SealError, TokenKind};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The longest a sign-in may take between `/console/login` and its callback.
pub const CONSOLE_LOGIN_TTL: i64 = 600;

/// One sign-in attempt: the OAuth `state`, the PKCE verifier and where to go
/// afterwards.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleLogin {
    pub state: String,
    pub verifier: String,
    pub return_to: String,
    /// Unix seconds after which the attempt is refused.
    pub exp: i64,
}

/// `state` and the verifier bind the authorization code to this browser, so
/// neither is ever formatted.
impl fmt::Debug for ConsoleLogin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ConsoleLogin")
            .field("exp", &self.exp)
            .finish_non_exhaustive()
    }
}

pub fn seal_console_login(ring: &MasterKeyRing, login: &ConsoleLogin) -> Result<String, SealError> {
    seal(ring, TokenKind::ConsoleLogin, login)
}

/// Open a login cookie, refusing it once `now` reaches its expiry or when
/// its window exceeds [`CONSOLE_LOGIN_TTL`].
pub fn open_console_login(
    ring: &MasterKeyRing,
    token: &str,
    now: i64,
) -> Result<ConsoleLogin, OpenError> {
    let login: ConsoleLogin = open(ring, TokenKind::ConsoleLogin, token)?.payload;
    if login.exp > now.saturating_add(CONSOLE_LOGIN_TTL) {
        return Err(OpenError::Format);
    }
    if now >= login.exp {
        return Err(OpenError::Expired);
    }
    Ok(login)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seal::{seal as seal_token, TokenKind};
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    fn ring() -> MasterKeyRing {
        MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([3; 32]))).unwrap()
    }

    fn login(exp: i64) -> ConsoleLogin {
        ConsoleLogin {
            state: "state-fixture".into(),
            verifier: "verifier-fixture".into(),
            return_to: "/console".into(),
            exp,
        }
    }

    #[test]
    fn a_login_round_trips_until_it_expires() {
        let ring = ring();
        let token = seal_console_login(&ring, &login(1_600)).unwrap();
        assert!(token.starts_with("cmaeg1.cl.1."));
        assert_eq!(
            open_console_login(&ring, &token, 1_000).unwrap(),
            login(1_600)
        );
        assert!(matches!(
            open_console_login(&ring, &token, 1_600),
            Err(OpenError::Expired)
        ));
        assert!(!format!("{:?}", login(1)).contains("fixture"));
    }

    #[test]
    fn other_kinds_long_windows_and_tampering_are_refused() {
        let ring = ring();
        let other = seal_token(&ring, TokenKind::ConnectionSetup, &login(1_600)).unwrap();
        assert!(matches!(
            open_console_login(&ring, &other, 1_000),
            Err(OpenError::WrongKind)
        ));
        let long = seal_console_login(&ring, &login(1_601)).unwrap();
        assert!(matches!(
            open_console_login(&ring, &long, 1_000),
            Err(OpenError::Format)
        ));
        let token = seal_console_login(&ring, &login(1_600)).unwrap();
        let mut tampered = token.into_bytes();
        let last = tampered.len() - 2;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
        assert!(open_console_login(&ring, &String::from_utf8(tampered).unwrap(), 1_000).is_err());
    }
}

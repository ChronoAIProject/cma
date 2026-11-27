//! Minting the secrets this service issues.
//!
//! Its own module because everything here is one decision — where the
//! randomness comes from — and that decision protects three different things:
//! the session cookie, the OAuth `state`, and the PKCE verifier. Kept apart
//! from the flow that spends them so it can be read, and reviewed, on its own.

/// Length of a generated token, in hexadecimal characters.
///
/// Two v4 UUIDs' worth. The lower bound is set by PKCE rather than by the
/// cookie: RFC 7636 requires a verifier of 43 to 128 unreserved characters, so
/// a single UUID's 32 would be rejected by a conforming provider.
const TOKEN_HEX_CHARS: usize = 64;

/// A random, unguessable token.
///
/// Used for session cookies, OAuth `state` and PKCE verifiers. Every one of
/// those is a bearer secret: whoever can predict a session cookie is signed in
/// as its owner, and whoever can predict a verifier can complete somebody
/// else's authorization code exchange.
///
/// Drawn from the operating system's CSPRNG, which is what `Uuid::new_v4`
/// uses. Deriving one from the clock and a process address would be neither —
/// the clock is public to within the round trip that observes it, and address
/// layout is randomised per process rather than per call, so a single token
/// observed by an attacker would fix the non-time half of every later one.
pub fn random_token() -> String {
    let mut token = String::with_capacity(TOKEN_HEX_CHARS);
    while token.len() < TOKEN_HEX_CHARS {
        token.push_str(&uuid::Uuid::new_v4().simple().to_string());
    }
    token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_long_enough_for_the_strictest_thing_it_is_used_as() {
        // PKCE sets the bar, not the cookie: a verifier outside 43..=128
        // unreserved characters is rejected by a conforming provider, so a
        // shorter token would break sign-in rather than merely weaken it.
        let token = random_token();
        assert_eq!(token.len(), TOKEN_HEX_CHARS);
        assert!(
            (43..=128).contains(&token.len()),
            "outside the PKCE verifier length RFC 7636 allows: {}",
            token.len()
        );
        assert!(
            token.chars().all(|c| c.is_ascii_hexdigit()),
            "a verifier may only carry unreserved characters"
        );
    }

    #[test]
    fn a_token_carries_no_fixed_run_an_observer_could_learn() {
        // The failure this replaced: a token built from the clock and a stack
        // address repeated that address on every call, so one token observed
        // by an attacker fixed a sixteen-character tail of every later one.
        // Uniqueness never caught it, because the clock moved — so what is
        // asserted is that no position is constant across a large batch,
        // beyond the one nibble per UUID that names its own version.
        const SAMPLES: usize = 256;
        let tokens: Vec<String> = (0..SAMPLES).map(|_| random_token()).collect();
        assert_eq!(
            tokens
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            SAMPLES,
            "tokens repeated"
        );

        let constant: Vec<usize> = (0..TOKEN_HEX_CHARS)
            .filter(|&position| {
                tokens
                    .iter()
                    .filter_map(|t| t.as_bytes().get(position))
                    .collect::<std::collections::HashSet<_>>()
                    .len()
                    == 1
            })
            .collect();
        assert_eq!(
            constant,
            vec![12, 44],
            "only each UUID's version nibble may be constant; anything else is \
             a run of the token that is not secret"
        );
    }
}

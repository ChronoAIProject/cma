use super::acp::Outcome;

const CONTINUATION: &str = "The failed turn will not be replayed automatically. Earlier work may have completed; review the conversation before sending a new message to continue.";

pub(super) fn normalize(message: &str, bounded_json: bool) -> Outcome {
    if let Some(credit) = super::acp_credit_failure::classify(message, bounded_json) {
        return credit;
    }
    let (class, summary) = match status(message) {
        Some(408 | 504 | 522 | 524) => {
            ("upstream_timeout", "The upstream model service timed out.")
        }
        Some(500..=599) => (
            "upstream_unavailable",
            "The upstream model service is temporarily unavailable.",
        ),
        Some(429) => (
            "upstream_rate_limited",
            "The upstream model service is limiting requests. Wait before sending another message.",
        ),
        _ => ("agent_turn_failed", "The Agent run failed."),
    };
    Outcome::Failed {
        capability: Some(class.into()),
        message: format!("{summary} {CONTINUATION}"),
    }
}

pub(super) fn unknown() -> Outcome {
    normalize("", true)
}

fn status(message: &str) -> Option<u16> {
    // Inspect only the transport prefix. The remainder is an opaque body.
    let prefix = &message.as_bytes()[..message.len().min(128)];
    let leading = prefix
        .iter()
        .take(9)
        .position(|b| !b.is_ascii_whitespace())?;
    let tail = prefix.get(leading..)?.strip_prefix(b"unexpected status ")?;
    let digits = tail.get(..3)?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    match tail.get(3)? {
        b':' => (),
        b' ' => {
            let reason = tail.get(4..)?.split(|b| *b == b':').next()?;
            if tail.get(4 + reason.len()) != Some(&b':')
                || !reason.first().is_some_and(u8::is_ascii_alphabetic)
                || !reason
                    .iter()
                    .all(|b| b.is_ascii_alphabetic() || matches!(b, b' ' | b'-'))
            {
                return None;
            }
        }
        _ => return None,
    }
    Some(
        digits
            .iter()
            .fold(0, |n, digit| n * 10 + u16::from(digit - b'0')),
    )
}

#[cfg(test)]
#[path = "acp_provider_failure_tests.rs"]
mod tests;

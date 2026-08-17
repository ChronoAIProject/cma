//! What a conversation's envelope carries, and what it refuses to.

use super::*;

#[test]
fn a_sealed_conversation_carries_its_path_and_its_bytes_unchanged() {
    // Byte-exact on the payload is the whole contract. Anything that rewrote
    // the agent's own file is something that breaks when the agent changes it.
    let body = b"{\"turn\":1}\n{\"turn\":2}\n";
    let sealed = seal(Path::new("--home-chrono--/abc/session.jsonl"), body);
    let (path, payload) = open(&sealed);
    assert_eq!(
        path.as_deref(),
        Some(Path::new("--home-chrono--/abc/session.jsonl"))
    );
    assert_eq!(payload, body);
}

#[test]
fn a_payload_that_is_not_text_at_all_survives_the_round_trip() {
    // One kernel writes newline-delimited JSON and another compressed frames.
    // The header is ours and is text; what follows it is neither read nor
    // assumed to be.
    let body = &[0x28, 0xb5, 0x2f, 0xfd, 0x00, b'\n', 0x01, 0x02];
    let sealed = seal(Path::new("a/b/session.jsonl.zstd"), body);
    let (path, payload) = open(&sealed);
    assert_eq!(path.as_deref(), Some(Path::new("a/b/session.jsonl.zstd")));
    assert_eq!(payload, body);
}

#[test]
fn bytes_from_a_pod_that_predates_the_envelope_are_carried_whole() {
    // The §5 half. A rollout is in flight between pods of different vintages
    // exactly when this matters most, and an unfamiliar payload is the older
    // dialect rather than a failure.
    let raw = b"{\"timestamp\":\"2026-08-14T00:00:00Z\",\"type\":\"session_meta\"}\n{\"turn\":1}\n";
    let (path, payload) = open(raw);
    assert!(path.is_none(), "an older pod names no path");
    assert_eq!(payload, raw, "and its conversation must not be truncated");
}

#[test]
fn an_agents_own_first_line_is_not_mistaken_for_a_header() {
    // The marker names this service for exactly this reason: a rollout whose
    // first line happens to be JSON must not have that line eaten.
    let raw = br#"{"payload":{"id":"019ed5ef"},"type":"session_meta"}
{"turn":1}
"#;
    let (path, payload) = open(raw);
    assert!(path.is_none());
    assert_eq!(payload, raw);
}

#[test]
fn a_header_from_a_newer_envelope_is_not_guessed_at() {
    // A version this build does not implement means the header's meaning may
    // have changed. The payload is still carried; the placement falls back.
    let sealed = format!("{{\"{MARKER}\":99,\"path\":\"a/b\"}}\nbody");
    let (path, payload) = open(sealed.as_bytes());
    assert!(path.is_none());
    assert_eq!(payload, b"body");
}

#[test]
fn a_path_that_climbs_out_of_the_store_is_refused() {
    // The bytes crossed an HTTP hop from a pod. A path taken on trust is a
    // write to anywhere this process can reach.
    for escape in [
        "../../etc/passwd",
        "a/../../b",
        "/etc/passwd",
        "",
        ".",
        "./..",
    ] {
        let sealed = seal(Path::new(escape), b"body");
        let (path, payload) = open(&sealed);
        assert!(path.is_none(), "{escape} must not be trusted");
        assert_eq!(payload, b"body", "the conversation is still carried");
    }
}

#[test]
fn a_path_that_merely_says_here_is_kept_rather_than_refused() {
    let sealed = seal(Path::new("./sessions/session.jsonl"), b"body");
    let (path, _) = open(&sealed);
    assert_eq!(path.as_deref(), Some(Path::new("sessions/session.jsonl")));
}

#[test]
fn a_header_with_no_path_at_all_still_yields_its_conversation() {
    let sealed = format!("{{\"{MARKER}\":{VERSION}}}\nbody");
    let (path, payload) = open(sealed.as_bytes());
    assert!(path.is_none());
    assert_eq!(payload, b"body");
}

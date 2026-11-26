use super::*;

fn auth(value: u8) -> Authentication {
    Authentication::new(hmac::Key::new(hmac::HMAC_SHA256, &[value; 32]))
}

#[test]
fn request_is_bound_to_recipient_and_cannot_replay() {
    let sender = auth(7);
    let recipient = auth(7);
    let (_, raw) = sender.request("a", "b", "operator").unwrap();
    assert!(recipient.verify_request(&raw, "c").is_err());
    let proof = recipient.verify_request(&raw, "b").unwrap();
    assert_eq!(proof.sender, "a");
    assert_eq!(proof.principal, "operator");
    assert!(recipient.verify_request(&raw, "b").is_err());
}

#[test]
fn proof_refuses_wrong_key_tampering_expiry_and_future_issue() {
    let sender = auth(7);
    let recipient = auth(7);
    let (proof, raw) = sender.request("a", "b", "operator").unwrap();
    assert!(auth(8).verify_request(&raw, "b").is_err());
    assert!(recipient
        .verify_request_at(&raw, "b", proof.expires_at)
        .is_err());
    assert!(recipient
        .verify_request_at(&raw, "b", proof.issued_at - 1)
        .is_err());
    let (encoded, signature) = raw.split_once('.').unwrap();
    let mut changed = proof.clone();
    changed.principal = "other-operator".into();
    let altered = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&changed).unwrap()),
        signature
    );
    assert!(recipient.verify_request(&altered, "b").is_err());
    assert!(recipient
        .verify_request(&format!("{encoded}.AA"), "b")
        .is_err());
    assert!(recipient
        .verify_request(&"x".repeat(MAX_PROOF_BYTES + 1), "b")
        .is_err());
}

#[test]
fn response_authenticates_bytes_and_exact_request_context() {
    let sender = auth(7);
    let recipient = auth(7);
    let (proof, raw) = sender.request("a", "b", "operator").unwrap();
    let accepted = recipient.verify_request(&raw, "b").unwrap();
    let signature = recipient.response(&accepted, b"observation").unwrap();
    sender
        .verify_response(&proof, b"observation", &signature)
        .unwrap();
    assert!(sender
        .verify_response(&proof, b"altered", &signature)
        .is_err());
    let (different, _) = sender.request("a", "b", "operator").unwrap();
    assert!(sender
        .verify_response(&different, b"observation", &signature)
        .is_err());
    let (_, request_signature) = raw.split_once('.').unwrap();
    assert!(sender
        .verify_response(&proof, b"observation", request_signature)
        .is_err());
}

#[test]
fn replay_memory_is_bounded_and_expired_entries_are_removed() {
    let sender = auth(7);
    let recipient = auth(7);
    let (proof, raw) = sender.request("a", "b", "operator").unwrap();
    {
        let mut nonces = recipient.nonces.lock().unwrap();
        for index in 0..MAX_NONCES {
            nonces.insert(index.to_string(), proof.expires_at);
        }
    }
    assert!(recipient
        .verify_request_at(&raw, "b", proof.issued_at)
        .is_err());
    recipient
        .nonces
        .lock()
        .unwrap()
        .values_mut()
        .for_each(|expiry| *expiry = proof.issued_at);
    assert!(recipient
        .verify_request_at(&raw, "b", proof.issued_at)
        .is_ok());
    assert_eq!(recipient.nonces.lock().unwrap().len(), 1);
}

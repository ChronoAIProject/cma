//! A sender signs exactly the request ID the receiver's request context keeps.
use super::*;

fn owner() -> OwnershipToken {
    OwnershipToken {
        agent_id: layer2_core::AgentId("agt-signing".into()),
        instance_id: "instance-receiver".into(),
        epoch: 3,
        protocol: layer2_core::agent_ownership::PROTOCOL_VERSION,
    }
}

#[test]
fn signed_request_ids_match_the_receivers_normalized_view() {
    const CANONICAL: &str = "00000000-0000-4000-8000-0000000000b1";
    let authentication = Authentication::new(hmac::Key::new(
        hmac::HMAC_SHA256,
        b"synthetic-relay-signing-key",
    ));
    let uri: Uri = "/internal/agent-manager/v1/live/agt-signing"
        .parse()
        .unwrap();
    let padded = format!("  {CANONICAL}\t");
    let uppercase = CANONICAL.to_uppercase();
    for (inputs, kept) in [
        // Usable but non-canonical: the receiver trims it, so the sender must too.
        (vec![padded.as_str()], Some(CANONICAL)),
        (vec![CANONICAL], Some(CANONICAL)),
        // Unusable values and duplicates are replaced by a fresh ID.
        (vec![uppercase.as_str()], None),
        (vec!["not-a-request-id"], None),
        (vec![CANONICAL, CANONICAL], None),
        (vec![], None),
    ] {
        let mut headers = HeaderMap::new();
        for value in &inputs {
            headers.append(REQUEST_ID, value.parse().unwrap());
        }
        let signed = authentication
            .signed_headers(
                "instance-sender",
                &owner(),
                &Method::GET,
                &uri,
                headers,
                &[],
            )
            .unwrap();
        let ids: Vec<_> = signed.get_all(REQUEST_ID).iter().collect();
        assert_eq!(ids.len(), 1, "{inputs:?}");
        let id = ids[0].to_str().unwrap();
        match kept {
            Some(kept) => assert_eq!(id, kept, "{inputs:?}"),
            None => assert!(!inputs.contains(&id), "{inputs:?} kept {id}"),
        }
        // The receiver's request context keeps this exact value unchanged.
        assert_eq!(
            layer1_public::rest::request_id_from_headers(&signed),
            Some(id),
            "{inputs:?}"
        );
        let proof = signed[PROOF].to_str().unwrap().to_owned();
        let verified = authentication
            .verify(&proof, &Method::GET, &uri, &signed, &[])
            .unwrap();
        assert_eq!(verified.sender, "instance-sender");
        assert_eq!(verified.owner, owner());
    }
}

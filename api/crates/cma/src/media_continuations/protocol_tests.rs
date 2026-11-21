use super::*;
use cmaeg_core::media_continuation as gateway;
use serde_json::json;

#[test]
fn manager_media_wire_and_exact_text_match_gateway_for_each_kind() {
    for kind in [
        gateway::MediaContinuationKind::Original,
        gateway::MediaContinuationKind::Transcript,
        gateway::MediaContinuationKind::TextExcerpt,
    ] {
        let derived = if kind == gateway::MediaContinuationKind::Original {
            ""
        } else {
            "  exact UTF-8: 世界\n"
        };
        let source = gateway::sha256(b"original bytes");
        let claim = gateway::MediaArtifactClaim {
            attachment_index: 31,
            kind,
            content_sha256: if derived.is_empty() {
                source.clone()
            } else {
                gateway::sha256(derived.as_bytes())
            },
            source_sha256: source,
            name: "audio-1.ogg".into(),
            media_type: "audio/ogg".into(),
            size_bytes: 14,
            text_sha256: None,
        };
        let wire = serde_json::to_value(&claim).unwrap();
        let local: MediaArtifactClaim = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(&local).unwrap(), wire);
        let path = "/home/attachments/agt_contract/att_contract/audio-1.ogg";
        assert_eq!(
            supplement_text(&local, path, derived).unwrap(),
            gateway::supplement_text(&claim, path, derived).unwrap()
        );
        assert_eq!(
            serde_json::to_value(local.original()).unwrap(),
            serde_json::to_value(claim.original()).unwrap()
        );
        for field in [
            "content_sha256",
            "source_sha256",
            "text_sha256",
            "name",
            "media_type",
        ] {
            let mut invalid = wire.clone();
            invalid[field] = json!("invalid/unsafe\n");
            let local: MediaArtifactClaim = serde_json::from_value(invalid.clone()).unwrap();
            let remote: gateway::MediaArtifactClaim = serde_json::from_value(invalid).unwrap();
            assert!(local.validate().is_err());
            assert!(remote.validate().is_err());
        }
    }
}
#[test]
fn retained_complete_activity_digest_matches_gateway_and_changes_with_nested_context() {
    let activity: cmaeg_core::activity::Activity = serde_json::from_value(json!({
        "schema_version":1,"event_id":"media-contract","kind":{"type":"message","text":"inspect","mentions_bot":false,"attachments":[{"reference":"original","kind":"file","filename":"original.bin","mime_type":"application/octet-stream","size_bytes":14,"source":{"type":"url","url":"https://source.example/file"}}]},
        "actor":{"id":"sender","kind":"human"},"conversation":{"id":"transport","kind":"custom"},"occurred_at":"2026-10-29T00:00:00Z","source":{"type":"custom"}
    })).unwrap();
    activity.validate().unwrap();
    let mut wire = serde_json::to_value(&activity).unwrap();
    let digest = activity_digest(wire.clone());
    assert_eq!(
        digest,
        cmaeg_core::seal::event_ref::activity_digest(&activity)
    );
    wire["kind"]["attachments"][0]["source"]["url"] = json!("https://source.example/changed");
    assert_ne!(activity_digest(wire), digest);
}

use super::*;
use crate::sandbox::AgentTranscriptPageRequest;

#[tokio::test]
async fn retained_provider_failure_is_safe_on_owner_and_shared_read_wrappers() {
    let (store, _, access, selector) = setup(Reply::Role(OrganizationRole::Viewer)).await;
    let agent = history(&store).await;
    access
        .change_share(
            &context("owner"),
            Some("request-credential"),
            create(&selector, "grant"),
        )
        .await
        .unwrap();
    let reads = service(store.clone()).with_resource_access(Arc::new(access));
    let original = store.agent_log(&agent.id, 0, 200).await.unwrap();
    let before = serde_json::to_vec(&original).unwrap();
    let owner = context("owner").caller;
    for transcript in [
        reads.transcript(&owner, &agent.id).await.unwrap(),
        reads.transcript_bounded(&owner, &agent.id).await.unwrap(),
        reads
            .transcript_page(&owner, &agent.id, &first())
            .await
            .unwrap()
            .transcript,
    ] {
        safe(&serde_json::to_value(transcript).unwrap());
    }
    for subject in ["owner", "recipient"] {
        let transcript = reads
            .transcript_authorized(&context(subject), Some("request-credential"), &agent.id)
            .await
            .unwrap();
        safe(&serde_json::to_value(&transcript).unwrap());
        let mut page_request = first();
        let mut messages = vec![];
        loop {
            let page = reads
                .transcript_page_authorized(
                    &context(subject),
                    Some("request-credential"),
                    &agent.id,
                    &page_request,
                )
                .await
                .unwrap();
            safe(&serde_json::to_value(&page.transcript).unwrap());
            for turn in page.transcript.turns {
                messages.extend(turn.messages);
                if subject == "recipient" {
                    assert!(turn.origin_key.is_none() && turn.metadata.is_empty());
                }
            }
            if let Some(next) = page.next {
                page_request = next;
            } else {
                break;
            }
        }
        assert_eq!(
            messages,
            transcript
                .turns
                .iter()
                .flat_map(|t| t.messages.clone())
                .collect::<Vec<_>>()
        );
        let (visible, next) = reads
            .log_page_authorized(
                &context(subject),
                Some("request-credential"),
                &agent.id,
                0,
                200,
            )
            .await
            .unwrap();
        assert!(next.is_none());
        assert_eq!(visible.len(), original.len());
        for (visible, raw) in visible.iter().zip(&original) {
            assert_eq!(
                (visible.seq, &visible.appended_at),
                (raw.seq, &raw.appended_at)
            );
            if let Some(AgentLogKind::TurnFailed {
                failure,
                turn_sequence,
            }) = &visible.kind
            {
                assert_eq!(*turn_sequence, 3);
                safe(&serde_json::to_value(failure).unwrap());
                assert_eq!(failure.class, "agent_turn_failed");
            }
        }
    }
    assert_eq!(
        serde_json::to_vec(&store.agent_log(&agent.id, 0, 200).await.unwrap()).unwrap(),
        before
    );
    assert!(String::from_utf8(before).unwrap().contains("private-host"));
    assert!(reads
        .transcript(&context("stranger").caller, &agent.id)
        .await
        .is_err());
}

fn first() -> AgentTranscriptPageRequest {
    AgentTranscriptPageRequest {
        turn: None,
        after: None,
        through: None,
        limit: 2,
    }
}

fn safe(value: &serde_json::Value) {
    let wire = value.to_string();
    assert!(
        !wire.contains("private-host"),
        "retained body leaked: {wire}"
    );
    assert!(
        !wire.contains("runtime_failed"),
        "unapproved class leaked: {wire}"
    );
}

use super::*;
use crate::store::MongoBridgeAllocationStore;
use layer2_core::bridge::{allocation::*, ingress::*, partition::*};

fn admission(bridge: &BridgeRecord) -> AllocationAdmission {
    let connector = &bridge.current().unwrap().config.connector;
    let inbox = StoredInbox {
        id: "a".repeat(64),
        owner: bridge.owner.clone(),
        pin: bridge.admission_pin().unwrap(),
        bot_id: connector.bot_id.clone(),
        key_id: connector.key_id.clone(),
        message_id: "message-one".into(),
        correlation_id: "correlation-one".into(),
        envelope: InboxEnvelope {
            platform: "telegram".into(),
            route_id: "route".into(),
            chat_id: "chat-one".into(),
            conversation_type: "private".into(),
            sender_id: "sender-one".into(),
            sender_display_name: None,
            content_type: "text".into(),
            text: Some("hello".into()),
            attachments: vec![],
            reply_to_message_id: None,
            reply_to_platform_message_id: None,
            protected_thread_context_present: false,
            timestamp: "2026-09-15T06:00:00Z".into(),
        },
        attestation: CallbackAttestation {
            issuer: connector.issuer.clone(),
            signing_key_id: "signing".into(),
            token_id: "token".into(),
            issued_at: 100,
            expires_at: 200,
            body_sha256: "b".repeat(64),
        },
        relay: RelayCorroboration::AbsentInboxOnly,
        admission: InboxAdmission::Accepted,
        capabilities: None,
        text_attachment: None,
        execution_hold: None,
        accepted_at: 101,
        expires_at: 3601,
        reply_expires_at: None,
    };
    let mut observation = PlatformCapabilityObservation::unknown(&inbox).unwrap();
    observation.sender_kind = ChannelSenderKind::Human;
    observation.event_kind = ChannelEventKind::UserMessage;
    observation.loop_evidence = LoopEvidence::NotLoop;
    let source = NormalizedPartitionSource::from_observation(&inbox, bridge, observation).unwrap();
    AllocationAdmission::new(bridge.clone(), &source).unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn fixed_allocation_binds_one_retained_target_without_provisioning_and_refuses_revoked_grant()
{
    for revoke in [false, true] {
        let mut f = Fixture::new().await;
        let grant = f
            .service
            .issue(&caller(), None, f.issue("consent"))
            .await
            .unwrap()
            .result;
        f.enable("enable").await;
        let admission = admission(&f.bridge);
        let id = admission.identity().id().unwrap();
        let proof = f
            .service
            .authorize(f.request(
                &grant,
                BridgeInvokeEffect::BindExistingTarget { allocation_id: id },
            ))
            .await
            .unwrap();
        let authority = AllocationAuthority::Existing(Box::new(proof));
        let write = AllocationReservation::new(admission, authority, 1000).unwrap();
        let store =
            MongoBridgeAllocationStore::open(f.bridges.clone(), Some(f.store.clone()), &f.database)
                .await
                .unwrap();
        let target = ResourceTarget::Agent {
            agent_id: f.target.agent_id.clone(),
        };
        let before = f.resources.resource_access_snapshot(&target).await.unwrap();
        if revoke {
            f.service
                .revoke(&caller(), None, f.revoke(&grant, "revoke"))
                .await
                .unwrap();
        }
        let result = store.reserve(write).await;
        if revoke {
            assert!(matches!(result, Err(AllocationError::AuthorityUnavailable)));
        } else {
            let record = result.unwrap();
            assert_eq!(record.state, AllocationState::Ready);
            assert_eq!(record.placement.target(), &f.target);
            assert!(record.effect_started);
            assert!(record.receipt.as_ref().unwrap().profiles_digest.is_none());
            assert!(record.receipt.is_some());
        }
        assert_eq!(
            f.resources.resource_access_snapshot(&target).await.unwrap(),
            before
        );
        assert_eq!(
            f.resources
                .sandboxes
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            f.resources.agents.count_documents(doc! {}).await.unwrap(),
            1
        );
        f.close().await;
    }
}

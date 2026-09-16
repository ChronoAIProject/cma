use super::*;
use layer2_core::bridge::ingress::CapturedAttachmentText;
use sha2::{Digest, Sha256};

fn capture(text: &str) -> CapturedAttachmentText {
    CapturedAttachmentText {
        text: text.into(),
        sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
    }
}

async fn proof(c: &ChannelFixture) -> BridgeInvokeProof {
    c.f.service
        .authorize(c.f.request(
            &c.grant,
            BridgeInvokeEffect::AdmitMessage {
                inbox_id: c.inbox.id.clone(),
            },
        ))
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn discord_text_custody_capture_and_command_retention_are_scoped_and_immutable() {
    let mut c = ChannelFixture::discord_text().await;
    let pending = c.inbox.clone();
    let authority = proof(&c).await;
    let url = c
        .inbox_store
        .text_attachment_url(&pending, &authority, &c.f.store)
        .await
        .unwrap();
    assert!(url.starts_with("https://cdn.discordapp.com/attachments/"));
    let db = c.f.client.database(&c.f.database);
    let custody = db.collection::<Document>("bridge_text_attachments");
    let private = custody
        .find_one(doc! {"_id": &pending.id})
        .await
        .unwrap()
        .unwrap();
    assert!(!serde_json::to_string(&private)
        .unwrap()
        .contains(url.as_str()));
    let mut foreign = pending.clone();
    foreign.owner = ResourceOwner::Person {
        subject: layer2_core::SubjectId::new("other"),
    };
    assert!(c
        .inbox_store
        .text_attachment_url(&foreign, &authority, &c.f.store)
        .await
        .is_err());
    assert!(c
        .f
        .store
        .capture_text_attachment(&authority, &foreign, &capture("hello"))
        .await
        .is_err());
    c.inbox = crate::bridge::attachments::capture_test_io::capture(
        &pending,
        &authority,
        &c.f.service,
        &c.inbox_store,
        &c.f.store,
        b"hello",
    )
    .await
    .unwrap();
    let replay =
        c.f.store
            .capture_text_attachment(&authority, &pending, &capture("world"))
            .await
            .unwrap();
    assert!(replay == c.inbox);
    assert_eq!(
        replay
            .text_attachment
            .as_ref()
            .unwrap()
            .captured
            .as_ref()
            .unwrap()
            .text,
        "hello"
    );
    assert_eq!(replay.attestation, pending.attestation);
    assert_eq!(replay.id, pending.id);
    let request = c.request().await;
    let command = c
        .commands
        .reserve_channel_command(request.clone())
        .await
        .unwrap();
    assert_eq!(
        c.commands
            .reserve_channel_command(c.request().await)
            .await
            .unwrap(),
        command
    );
    let AgentCommand::Message { input, .. } = &command.receipt.envelope.command else {
        panic!("message required");
    };
    let AgentContent::Text { text } = &input.content[0] else {
        panic!("text required");
    };
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(value["caption"], "hello");
    assert_eq!(value["attachment"]["untrusted_text"], "hello");
    assert!(!text.contains("cdn.discordapp.com"));
    assert_eq!(
        db.collection::<Document>("agent_commands")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    // Logical file expiry is checked even if Mongo has not removed ciphertext.
    custody
        .update_one(
            doc! {"_id": &pending.id},
            doc! {"$set": {"expires_at": mongodb::bson::DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
    assert!(c
        .inbox_store
        .text_attachment_url(&pending, &authority, &c.f.store)
        .await
        .is_err());
    // Retained user text continues through the existing command archive after
    // the original transport and file custody are both physically removed.
    custody.delete_one(doc! {"_id": &pending.id}).await.unwrap();
    db.collection::<Document>("bridge_callback_inbox")
        .delete_one(doc! {"_id": &pending.id})
        .await
        .unwrap();
    let service = ChannelAgentCommandService::new(c.commands.clone());
    let execution = service
        .authorize_execution(&c.f.service, &command)
        .await
        .unwrap();
    execution
        .fence()
        .require_envelope(&command.receipt.envelope)
        .unwrap();
    let archived = c.commands.channel_execution_source(&command).await.unwrap();
    assert!(archived.inbox == c.inbox);
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn discord_text_revoked_grant_cannot_open_custody_or_capture_with_a_held_proof() {
    let c = ChannelFixture::discord_text().await;
    let authority = proof(&c).await;
    c.f.service
        .revoke(
            &c.f.context,
            Some("fixture-credential"),
            c.f.revoke(&c.grant, "revoke-text-file"),
        )
        .await
        .unwrap();
    assert!(c
        .inbox_store
        .text_attachment_url(&c.inbox, &authority, &c.f.store)
        .await
        .is_err());
    assert!(c
        .f
        .store
        .capture_text_attachment(&authority, &c.inbox, &capture("hello"))
        .await
        .is_err());
    assert_eq!(
        c.f.client
            .database(&c.f.database)
            .collection::<Document>("agent_commands")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn discord_text_enabling_retrieval_never_reclassifies_a_durable_old_refusal() {
    use layer2_core::bridge::{ingress::*, partition::*};
    struct Disabled;
    impl CallbackCapabilityExtractor for Disabled {
        fn contract(&self) -> CapabilityContract {
            CapabilityContract::NyxidPlatformV1
        }
        fn observe(
            &self,
            p: &AuthenticatedCallbackPayload<'_>,
        ) -> Result<PlatformCapabilityObservation, CallbackError> {
            let mut facts = crate::bridge::ingress::NyxidCapabilities.observe(p)?;
            if facts.event_kind == ChannelEventKind::DiscordTextAttachment {
                facts.event_kind = ChannelEventKind::AttachmentMessage;
            }
            Ok(facts)
        }
    }
    let c = ChannelFixture::discord_text().await;
    let mut fixture = crate::bridge::attachments::discord_tests::fixture();
    fixture.bridge = c.f.bridge.clone();
    let config = &fixture.bridge.current().unwrap().config;
    fixture.body["agent"]["api_key_id"] = serde_json::json!(config.connector.key_id);
    fixture.body["conversation"]["id"] = serde_json::json!(config.connector.route_ids[0]);
    fixture.body["message_id"] = serde_json::json!("old-refused-text-file");
    let mut original = None;
    for enabled in [false, true] {
        let validation =
            c.f.authority
                .validate_connection(&c.f.bridge.owner, &c.f.bridge.current().unwrap().config)
                .await
                .unwrap();
        let extractor: &dyn CallbackCapabilityExtractor = if enabled {
            &crate::bridge::ingress::NyxidCapabilities
        } else {
            &Disabled
        };
        let write = fixture
            .signed()
            .unwrap()
            .classify_capabilities(&fixture.bytes(), extractor)
            .unwrap()
            .bind(CallbackBinding::new(c.f.bridge.clone(), validation).unwrap())
            .unwrap();
        let id = write.record().id.clone();
        let receipt = c.inbox_store.accept(write).await.unwrap();
        if let Some(original) = &original {
            assert_eq!(&receipt, original);
        } else {
            original = Some(receipt.clone());
        }
        let CallbackAcceptance::Message(receipt) = receipt else {
            panic!("message refusal required");
        };
        assert_eq!(
            receipt.admission,
            InboxAdmission::PartitionDenied {
                reason: PartitionRefusal::AttachmentDownloadRequired
            }
        );
        assert!(c.inbox_store.channel_input(&id).await.unwrap().is_none());
        assert!(c
            .f
            .client
            .database(&c.f.database)
            .collection::<Document>("bridge_text_attachments")
            .find_one(doc! {"_id": &id})
            .await
            .unwrap()
            .is_none());
    }
    c.close().await;
}

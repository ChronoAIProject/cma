use super::*;
use layer2_core::resource_access::{ResourceAccessStore, ResourceTarget};
use layer2_core::sandbox::store::{AgentStore, SandboxMutationGuard};

async fn finish_command(f: &ReplyFixture, command: &AdmittedAgentCommand) {
    f.finish().await;
    f.resources
        .reconcile_command(&command.receipt.envelope.command_id)
        .await
        .unwrap();
}

async fn claim(f: &ReplyFixture) -> AdmittedAgentCommand {
    let now = std::time::SystemTime::now();
    f.resources
        .claim_next_command(CommandClaimRequest {
            agent_id: f.command.receipt.envelope.agent_id.clone(),
            holder: "interleaved-input-worker".into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .expect("next admitted input must remain dispatchable")
}

async fn website_turn(
    f: &ReplyFixture,
    operations: &AgentOperationService,
    human_ui: bool,
) -> AdmittedAgentCommand {
    let agent_id = f.command.receipt.envelope.agent_id.clone();
    let snapshot = f
        .resources
        .resource_access_snapshot(&ResourceTarget::Agent {
            agent_id: agent_id.clone(),
        })
        .await
        .unwrap();
    let agent = f.resources.get_agent(&agent_id).await.unwrap().unwrap();
    let now = std::time::SystemTime::now();
    let command = AgentCommand::message("Owner request from the website");
    f.resources
        .reserve_command(CommandReservation {
            envelope: AgentCommandEnvelope {
                schema_version: AGENT_IO_SCHEMA_VERSION,
                command_id: layer2_core::ids::AgentCommandId::generate(),
                agent_id,
                owner: f.command.receipt.envelope.owner.clone(),
                actor: if human_ui {
                    caller().actor
                } else {
                    AgentActor::Api {
                        subject: caller().caller.subject,
                        delegation_id: None,
                    }
                },
                source: AgentCommandSource {
                    kind: if human_ui {
                        AgentSourceKind::UserInterface
                    } else {
                        AgentSourceKind::Api
                    },
                    reference: "website-interleaving".into(),
                },
                correlation_id: "website-interleaving".into(),
                request_fingerprint: command.fingerprint().unwrap(),
                authorization: AgentAuthorizationRevision {
                    authority_id: "personal-owner".into(),
                    policy_revision: 1,
                    grant_revision: None,
                },
                command,
            },
            key_sha256: command_key_digest("website-interleaving").unwrap(),
            response_id: Some(layer2_core::ids::AgentResponseId::generate()),
            admitted_at: layer2_core::rfc3339_at(now),
            queue_deadline: layer2_core::rfc3339_at(now + Duration::from_secs(300)),
            queue_capacity: 8,
            expected_agent_revision: agent.revision,
            parent: SandboxMutationGuard::capture(&snapshot.lineage.sandbox),
            policy: snapshot.fence(),
            authority_deadline: None,
            admission_audit: None,
            binding: None,
        })
        .await
        .unwrap();
    let claimed = claim(f).await;
    operations
        .execute_claimed_command(&caller().caller, &claimed)
        .await
        .unwrap();
    f.resources
        .get_command(&claimed.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap()
}

async fn dispatch_channel(
    f: &ReplyFixture,
    channel: &ChannelAgentCommandService,
    held: &AdmittedAgentCommand,
    operations: &AgentOperationService,
) {
    let authority = channel.authorize_execution(&f.invokes, held).await.unwrap();
    channel
        .release_execution_hold(&authority, held)
        .await
        .unwrap();
    let claimed = claim(f).await;
    assert_eq!(claimed.receipt, held.receipt);
    let authority = channel
        .authorize_execution(&f.invokes, &claimed)
        .await
        .unwrap();
    operations
        .execute_channel_claim(&authority, &claimed)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_supersession_website_interrupt_and_completion_keep_same_writer_dispatchable() {
    let f = ReplyFixture::private().await;
    let (operations, writes) = operations(&f);
    operations
        .execute_command(
            &caller().caller,
            &f.command.receipt.envelope.agent_id,
            AgentCommand::Interrupt,
        )
        .await
        .unwrap();
    finish_command(&f, &f.command).await;
    let website = website_turn(&f, &operations, false).await;
    finish_command(&f, &website).await;
    let (channel, held) = admit(&f, "telegram-after-website-completed").await;
    dispatch_channel(&f, &channel, &held, &operations).await;
    assert!(matches!(
        writes.lock().unwrap().as_slice(),
        [
            ConversationInput::Cancel,
            ConversationInput::RecoverablePrompt { .. },
            ConversationInput::RecoverablePrompt { .. }
        ]
    ));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_supersession_private_telegram_interrupts_exact_owner_website_turn_once() {
    for human_ui in [false, true] {
        let f = ReplyFixture::private().await;
        finish_command(&f, &f.command).await;
        let (operations, writes) = operations(&f);
        let website = website_turn(&f, &operations, human_ui).await;
        let (channel, held) = admit(&f, "telegram-during-owner-website-turn").await;
        let authority = channel
            .authorize_execution(&f.invokes, &held)
            .await
            .unwrap();
        let (first, duplicate) = tokio::join!(
            operations.interrupt_channel_for_input(&authority, &held),
            operations.interrupt_channel_for_input(&authority, &held),
        );
        assert!(
            first.is_ok() || duplicate.is_ok(),
            "{first:?}, {duplicate:?}"
        );
        assert!(matches!(
            writes.lock().unwrap().as_slice(),
            [
                ConversationInput::RecoverablePrompt { .. },
                ConversationInput::Cancel
            ]
        ));
        assert_eq!(
            f.resources
                .get_command(&held.receipt.envelope.command_id)
                .await
                .unwrap()
                .unwrap(),
            held,
        );
        let row = f
            .client
            .database(&f.database)
            .collection::<Document>("agent_commands")
            .find_one(doc! {"_id":held.receipt.envelope.command_id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let supersession = row.get_document("channel_supersession").unwrap();
        assert_eq!(
            supersession.get_str("source_command_id").unwrap(),
            website.receipt.envelope.command_id.as_str()
        );
        assert_eq!(
            supersession.get_str("channel_source_command_id").unwrap(),
            f.command.receipt.envelope.command_id.as_str()
        );
        finish_command(&f, &website).await;
        dispatch_channel(&f, &channel, &held, &operations).await;
        assert!(matches!(
            writes.lock().unwrap().as_slice(),
            [
                ConversationInput::RecoverablePrompt { .. },
                ConversationInput::Cancel,
                ConversationInput::RecoverablePrompt { .. }
            ]
        ));
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_supersession_website_exception_rejects_unrelated_actor_or_channel_anchor() {
    for case in [
        "trigger",
        "mismatched-source",
        "delegated-api",
        "foreign-human",
        "missing-anchor",
        "other-sender",
    ] {
        let f = ReplyFixture::private().await;
        finish_command(&f, &f.command).await;
        let (operations, writes) = operations(&f);
        let mut website = website_turn(&f, &operations, false).await;
        let commands = f
            .client
            .database(&f.database)
            .collection::<Document>("agent_commands");
        match case {
            "trigger" => {
                website.receipt.envelope.actor = AgentActor::Trigger {
                    subject: caller().caller.subject,
                    launch_id: layer2_core::LaunchId::generate(),
                };
                website.receipt.envelope.source.kind = AgentSourceKind::Trigger;
            }
            "mismatched-source" => {
                website.receipt.envelope.source.kind = AgentSourceKind::UserInterface
            }
            "delegated-api" => {
                website.receipt.envelope.actor = AgentActor::Api {
                    subject: caller().caller.subject,
                    delegation_id: Some("unrelated-delegation".into()),
                }
            }
            "foreign-human" => {
                website.receipt.envelope.actor = AgentActor::Human {
                    subject: layer2_core::SubjectId::new("another-owner"),
                };
                website.receipt.envelope.source.kind = AgentSourceKind::UserInterface;
            }
            "missing-anchor" => {
                commands
                    .update_one(
                        doc! {"_id":f.command.receipt.envelope.command_id.as_str()},
                        doc! {"$unset":{"channel_admission":""}},
                    )
                    .await
                    .unwrap();
            }
            "other-sender" => {
                commands
                    .update_one(
                        doc! {"_id":f.command.receipt.envelope.command_id.as_str()},
                        doc! {"$set":{"record.receipt.envelope.actor.sender_id":"another-sender"}},
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        commands
            .replace_one(
                doc! {"_id":website.receipt.envelope.command_id.as_str()},
                crate::store::agent_commands::command_document(&website).unwrap(),
            )
            .await
            .unwrap();
        let (channel, held) = admit(&f, "telegram-with-unrelated-active-source").await;
        let authority = channel
            .authorize_execution(&f.invokes, &held)
            .await
            .unwrap();
        assert!(
            operations
                .interrupt_channel_for_input(&authority, &held)
                .await
                .is_err(),
            "{case}"
        );
        assert!(
            matches!(
                writes.lock().unwrap().as_slice(),
                [ConversationInput::RecoverablePrompt { .. }]
            ),
            "{case}"
        );
        let row = commands
            .find_one(doc! {"_id":held.receipt.envelope.command_id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert!(!row.contains_key("channel_supersession"), "{case}");
        f.close().await;
    }
}

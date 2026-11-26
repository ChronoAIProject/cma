async fn profile_binding(store: &MongoSandboxStore, original: &ProviderBinding, id: &str, key: &str) -> ProviderBinding {
    let mut revision = original.clone();
    revision.binding_id = id.into();
    revision.key_sha256 = key.repeat(64);
    revision.revision = 1;
    revision.state = BindingState::Provisioning;
    revision.work = BindingWork::AwaitingCredential;
    store.save_binding(None, revision.clone()).await.unwrap();
    revision.state = BindingState::Ready;
    revision.work = BindingWork::Idle;
    revision.advance_revision(layer2_core::now_rfc3339()).unwrap();
    store.save_binding(Some(1), revision.clone()).await.unwrap();
    revision
}

fn profile_revision_fixture(owner: &SubjectId, prompt: &str) -> EventWorkerProfile {
    serde_json::from_value(json!({"schema_version":1,
        "metadata":{"name":"profile-fixture","description":"","owner":{"kind":"human","subject":owner},"provider":"cma_codex"},
        "model":{"access":"managed","service":"cma","api_type":"responses","model":"test","thinking_effort":"low"},
        "permission":{"action_mode":"guarded","service_scope":[]},
        "context":{"skills":[],"system_prompt":prompt,"mcp_servers":[]},"environment":null})).unwrap()
}

#[tokio::test]
#[ignore = "requires authenticated-mongo-contract.sh am and the compiled gateway"]
async fn profile_reset_receipt_replays_after_a_later_channel_revision() {
    use cmaeg::channels::definition::hash_secret;
    use cmaeg::connections::store::index_id;
    use cmaeg_core::seal::seal_connection;
    let (client, database, store, source, old) = super::fixture().await;
    let middle = profile_binding(&store, &source, "profile-middle-fixture", "b").await;
    let newest = profile_binding(&store, &source, "profile-newest-fixture", "c").await;
    let provider = Provider::start(store.clone(), &source).await;
    provider.enabled.store(true, Ordering::SeqCst);
    let mut gateway = Gateway::start(&provider, &client).await;
    let keys = MasterKeyRing::parse(&gateway.master).unwrap();
    let record = RecordId::new(uuid::Uuid::new_v4().to_string()).unwrap();
    let channel = cmaeg::channels::definition::channel_pointer(&provider.base, &record)
        .unwrap().seal_id(&keys).unwrap();
    let position = ConversationId::derive(&keys, &channel, 1, "profile-partition");
    store.bindings.conversations.delete_one(doc! {"_id":conversation_storage_key(&old)}).await.unwrap();
    let old = mapping(&store, &source, &old, position.to_string()).await;
    let credentials = ["profile-old-fixture", "profile-middle-fixture", "profile-newest-fixture"];
    let profiles = ["original instructions", "middle instructions", "newest instructions"]
        .map(|prompt| profile_revision_fixture(&source.owner, prompt));
    let bindings = [&source, &middle, &newest];
    let make_definition = |active: usize| {
        let current = &profiles[active];
        let retained = (0..active).map(|index| json!({
            "revision":index+1,"profile_digest":profiles[index].digest(),
            "profile":profiles[index],"provider":{"slug":"cma_codex","binding_id":bindings[index].binding_id,"level":"l1"},
            "credential":credentials[index],
        })).collect::<Vec<_>>();
        let definition: ChannelDefinition = serde_json::from_value(json!({
            "schema_version":1,"record_id":record,"owner":current.metadata.owner,
            "profile":current,"profile_digest":current.digest(),
            "provider":{"slug":"cma_codex","binding_id":bindings[active].binding_id,"level":"l1"},
            "sources":[],"partition":{"version":1,"key":"conversation_and_sender"},
            "reply":{"auto_final":false,"ack_after_ms":0,"sinks":[]},
            "secrets":{"agent_key_hash":hash_secret(credentials[active]),"provider_credential":credentials[active],
                "profile_revisions":{"current_revision":active+1,"baseline_complete":true,
                    "last_publication":{"expected_version":active,"request_fingerprint":hash_secret("profile-publication-fixture")},
                    "retained":retained}},
            "created_at":"2026-01-01T00:00:00Z","version":1,
        })).unwrap();
        definition.validate().unwrap();
        let blob = seal_blob(&keys, canonical_json(definition.to_blob_json()).as_bytes()).unwrap();
        doc! {"_id":record.as_str(),"owner":source.owner.as_str(),"version":i64::try_from(active+1).unwrap(),
            "schema_version":1_i32,"blob":mongodb::bson::Binary {subtype:mongodb::bson::spec::BinarySubtype::Generic,bytes:blob}}
    };
    let records = gateway.database.collection::<Document>("eg_channel_records");
    records.insert_one(make_definition(1)).await.unwrap();
    let assignment_id = index_id("cmaeg_partition_profile_v1", &[
        source.owner.as_str(), channel.as_str(), record.as_str(), "default", "cma_codex", position.as_str()
    ]);
    let assignment = json!({
        "kind":"cmaeg_partition_profile_v1","id":assignment_id,"owner":source.owner,
        "channel_id":channel.as_str(),"record_id":record.as_str(),"consumer":"default",
        "conversation_id":position.as_str(),"revision":1,"binding_id":source.binding_id,
        "profile_digest":profiles[0].digest(),"credential_fingerprint":hash_secret(credentials[0]),
        "first_revision":1,"incarnation":0
    });
    let blob = seal_connection(&keys, &assignment_id, &serde_json::to_vec(&assignment).unwrap()).unwrap();
    gateway.database.collection::<Document>("eg_connection_records").insert_one(doc! {
        "_id":&assignment_id,"owner":source.owner.as_str(),"version":1_i64,"schema_version":1_i32,
        "guard_revision":0_i64,"blob":mongodb::bson::Binary {subtype:mongodb::bson::spec::BinarySubtype::Generic,bytes:blob}
    }).await.unwrap();
    gateway.command.env("CMAEG_CHANNEL_PROFILE_REVISIONS_ENABLED", "true")
        .env("CMAEG_CHANNEL_MUTATION_CUSTODY_ENABLED", "true")
        .env("CMAEG_ROUTED_RECOVERY_ENABLED", "true")
        .env("CMAEG_ROUTED_ADMISSION_ENABLED", "true");
    gateway.restart().await;
    let body = json!({"partition_key":"profile-partition","expected_version":2,"expected_agent_id":old.agent_id});
    provider.lose_once.store(true, Ordering::SeqCst);
    gateway.reset(&channel, "profile-reset-fixture", &body, 503).await;
    let retired = store.binding_conversation(&source.binding_id, source.generation, position.as_str())
        .await.unwrap().unwrap();
    assert_eq!(retired.state, BindingConversationState::Retired);
    let committed = retired.profile_transfer.unwrap();
    assert_eq!(committed.request.destination_binding_id, middle.binding_id);
    records.replace_one(doc! {"_id":record.as_str(),"version":2_i64}, make_definition(2)).await.unwrap();
    gateway.restart().await;
    let first = gateway.reset(&channel, "profile-reset-fixture", &body, 200).await;
    let second = gateway.reset(&channel, "profile-reset-fixture", &body, 200).await;
    assert_eq!(first, second);
    assert_eq!(first["reset"]["binding_id"], middle.binding_id);
    assert_eq!(first["reset"]["successor_agent_id"], committed.destination_agent_id.as_str());
    assert_eq!(first["reset"]["incarnation"], 1);
    assert!(store.binding_conversation(&newest.binding_id,newest.generation,position.as_str()).await.unwrap().is_none());
    gateway.child.kill().await.unwrap();
    gateway.child.wait().await.unwrap();
    gateway.database.drop().await.unwrap();
    client.database(&database).drop().await.unwrap();
}

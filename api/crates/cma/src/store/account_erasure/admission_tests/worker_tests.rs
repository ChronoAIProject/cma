use super::*;

struct FixedBotProfiles(layer2_core::bots::BotProfiles);
#[async_trait::async_trait]
impl layer2_core::bots::BotProfileResolver for FixedBotProfiles {
    async fn resolve(
        &self,
        _: &SubjectId,
        _: &layer2_core::bots::CreateBot,
    ) -> Result<layer2_core::bots::BotProfiles, layer2_core::bots::BotError> {
        Ok(self.0.clone())
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn catalogue_bot_admission_fences_every_pin_and_replays_existing_receipts() {
    use layer2_core::bots::{BotProfiles, BotService};
    use layer2_core::trigger_domain::TriggerProfileReader;
    let seed = seeded("erasure_bot_catalogue_pins").await;
    let db = seed.client.database(&seed.database);
    let resolved = seed
        .profiles
        .read_trigger_profiles(&seed.target)
        .await
        .unwrap();
    let profiles = BotProfiles {
        event_worker_profile: None,
        resolved_model: "gpt-5".into(),
        agent: resolved.agent,
        sandbox: resolved.sandbox,
        workspace: resolved.workspace,
    };
    let store = Arc::new(
        crate::store::bots::MongoBotStore::open(&seed.client, &seed.database)
            .await
            .unwrap(),
    );
    let service = BotService::new(
        store,
        Arc::new(FixedBotProfiles(profiles)),
        "registered".into(),
    );
    let caller = Caller::new(
        SubjectId::new("visitor"),
        Authorization::new([Capability::ManageOwnBots]),
    );
    let mut request = crate::store::bots_mongo_integration_tests::bot("visitor", "a").request;
    request.profile = seed.target.agent_profile.clone();
    let admitted = service
        .create(&caller, "before-fence", request.clone())
        .await
        .unwrap();
    for (collection, id) in [
        ("sandbox_profiles", seed.target.sandbox_profile.id.as_str()),
        (
            "workspace_profiles",
            seed.target.workspace_profile.id.as_str(),
        ),
        ("agent_profiles", seed.target.agent_profile.id.as_str()),
    ] {
        db.collection::<Document>(collection)
            .update_one(
                doc! {"_id":id},
                doc! {"$set":{"account_erasure_fenced":true}},
            )
            .await
            .unwrap();
        assert!(service
            .create(&caller, collection, request.clone())
            .await
            .is_err());
        assert_eq!(
            service
                .create(&caller, "before-fence", request.clone())
                .await
                .unwrap(),
            admitted
        );
        assert_eq!(
            db.collection::<Document>("bots")
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
        db.collection::<Document>(collection)
            .update_one(
                doc! {"_id":id},
                doc! {"$unset":{"account_erasure_fenced":""}},
            )
            .await
            .unwrap();
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn event_worker_profile_publication_fences_catalogue_pins_and_preserves_replay() {
    use layer2_core::bots::BotProfiles;
    use layer2_core::event_workers::{EventWorkerProfileInput, EventWorkerProfileService};
    use layer2_core::trigger_domain::TriggerProfileReader;
    let seed = seeded("erasure_event_worker_pins").await;
    let db = seed.client.database(&seed.database);
    let resolved = seed
        .profiles
        .read_trigger_profiles(&seed.target)
        .await
        .unwrap();
    let profiles = BotProfiles {
        event_worker_profile: None,
        resolved_model: "gpt-5".into(),
        agent: resolved.agent,
        sandbox: resolved.sandbox,
        workspace: resolved.workspace,
    };
    let store = Arc::new(
        crate::store::event_workers::MongoEventWorkerProfileStore::open(
            &seed.client,
            &seed.database,
        )
        .await
        .unwrap(),
    );
    let service = EventWorkerProfileService::new(
        store,
        Arc::new(FixedBotProfiles(profiles)),
        "cma_codex".into(),
    );
    let caller = Caller::new(
        SubjectId::new("visitor"),
        Authorization::new([Capability::ManageOwnProfiles]),
    );
    let input = EventWorkerProfileInput {
        name: "Worker".into(),
        description: String::new(),
        agent_profile: seed.target.agent_profile.clone(),
        sandbox_profile: seed.target.sandbox_profile.clone(),
        workspace_profile: seed.target.workspace_profile.clone(),
    };
    let admitted = service
        .create(&caller, "before-fence", input.clone())
        .await
        .unwrap();
    for (collection, id) in [
        ("sandbox_profiles", seed.target.sandbox_profile.id.as_str()),
        (
            "workspace_profiles",
            seed.target.workspace_profile.id.as_str(),
        ),
        ("agent_profiles", seed.target.agent_profile.id.as_str()),
    ] {
        db.collection::<Document>(collection)
            .update_one(
                doc! {"_id":id},
                doc! {"$set":{"account_erasure_fenced":true}},
            )
            .await
            .unwrap();
        assert!(service
            .create(&caller, collection, input.clone())
            .await
            .is_err());
        assert_eq!(
            service
                .create(&caller, "before-fence", input.clone())
                .await
                .unwrap(),
            admitted
        );
        assert_eq!(
            db.collection::<Document>("event_worker_profiles")
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            db.collection::<Document>("event_worker_profile_revisions")
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
        db.collection::<Document>(collection)
            .update_one(
                doc! {"_id":id},
                doc! {"$unset":{"account_erasure_fenced":""}},
            )
            .await
            .unwrap();
    }
    db.drop().await.unwrap();
}

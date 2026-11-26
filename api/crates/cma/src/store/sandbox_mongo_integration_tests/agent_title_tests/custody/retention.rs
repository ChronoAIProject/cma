use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_replacement_fallback_off_and_malformed_floor_fail_closed() {
    let f = Fixture::new("retention").await;
    let (claim, intent) = f.intent().await;
    assert!(f.a.begin_agent_title_effect(&claim, &intent).await.unwrap());
    let before = f.row().await;
    let mut agent = f.b.get_agent(&f.id).await.unwrap().unwrap();
    let revision = agent.revision;
    agent.revision += 1;
    agent.updated_at = MINUTE.into();
    f.b.save_agent(revision, agent).await.unwrap();
    assert_eq!(
        f.row().await.get("title_effect_custody"),
        before.get("title_effect_custody")
    );
    assert!(!f
        .a
        .complete_agent_title(
            &claim,
            &AgentTitle::new("Fallback cannot erase intent").unwrap(),
            AgentTitleSource::Fallback,
            MINUTE.into()
        )
        .await
        .unwrap());
    assert!(peer(&f.name, false)
        .await
        .agent_title_effect(&f.id)
        .await
        .unwrap()
        .unknown());
    for field in ["title_effect_floor", "title_effect_custody"] {
        f.a.agents
            .update_one(doc! {"_id": f.id.as_str()}, doc! {"$unset": {field: ""}})
            .await
            .unwrap();
        let off = peer(&f.name, false).await;
        assert!(matches!(
            off.agent_title_effect(&f.id).await,
            Err(AgentTitleStoreError::Corrupt)
        ));
        assert_eq!(
            off.acquire_agent_title_claim(
                &AgentTitleClaimRequest::new(f.id.clone(), "cannot-downgrade"),
                SystemTime::now()
            )
            .await,
            Err(AgentTitleStoreError::Corrupt)
        );
        f.a.agents
            .update_one(
                doc! {"_id": f.id.as_str()},
                doc! {"$set": {field: before.get(field).unwrap()}},
            )
            .await
            .unwrap();
    }
    f.a.agents
        .update_one(
            doc! {"_id": f.id.as_str()},
            doc! {"$set": {"title_effect_custody": Bson::Null}},
        )
        .await
        .unwrap();
    assert!(matches!(
        f.b.agent_title_effect(&f.id).await,
        Err(AgentTitleStoreError::Corrupt)
    ));
    f.a.agents
        .update_one(
            doc! {"_id": f.id.as_str()},
            doc! {"$set": {"title_effect_custody": before.get("title_effect_custody").unwrap()}},
        )
        .await
        .unwrap();
    f.a.agents
        .update_one(
            doc! {"_id": f.id.as_str()},
            doc! {"$set": {"title_effect_custody.version": 99_i32}},
        )
        .await
        .unwrap();
    assert!(matches!(
        f.b.agent_title_effect(&f.id).await,
        Err(AgentTitleStoreError::Corrupt)
    ));
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_purge_requires_final_effect_and_permanent_identity() {
    let f = Fixture::new("purge").await;
    let (claim, intent) = f.intent().await;
    assert!(f.a.begin_agent_title_effect(&claim, &intent).await.unwrap());
    assert!(!purge_allowed(&f).await);
    let title = AgentTitle::new("Final provider result").unwrap();
    assert!(f
        .a
        .accept_agent_title_effect(&claim, &intent, &title)
        .await
        .unwrap());
    assert!(
        !purge_allowed(&f).await,
        "a result alone does not prove permanent identity retirement"
    );
    insert_provenance(&f).await;
    assert!(purge_allowed(&f).await);
    // Simulate the terminal purge after its transaction proved finality. The
    // retained provenance prevents both production admission and low-level
    // creation from recreating the same logical Agent identity.
    f.a.agents
        .delete_one(doc! {"_id": f.id.as_str()})
        .await
        .unwrap();
    let original = agent_for_worker(f.id.as_str(), NOW);
    assert!(matches!(
        f.b.create_agent(original).await,
        Err(SandboxStoreError::Duplicate)
    ));
    assert!(f
        .a
        .agent_creation_provenance
        .find_one(doc! {"_id": f.id.as_str()})
        .await
        .unwrap()
        .is_some());
    f.cleanup().await;
}
async fn insert_provenance(f: &Fixture) {
    let agent = f.a.get_agent(&f.id).await.unwrap().unwrap();
    let mut parent = sandbox(agent.sandbox_id.as_str());
    parent.owner = agent.owner.clone();
    let mut creation = agent_for_worker(f.id.as_str(), NOW);
    creation.conversation.last_seq = 1;
    let provenance = layer2_core::sandbox::AgentCreationProvenance::for_admission(
        &creation,
        &parent,
        layer2_core::resource_access::ResourceOwner::Person {
            subject: agent.owner.clone(),
        },
        None,
    )
    .unwrap();
    f.a.agent_creation_provenance
        .insert_one(crate::store::agent_provenance::document(&provenance).unwrap())
        .await
        .unwrap();
}

// #3275: nothing ever resolves an unresolved title effect, so purge finality
// must not wait for it once no capability can still accept a result.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_purge_accepts_an_unresolved_effect_after_its_claim_deadline() {
    let f = Fixture::new("purge_unresolved").await;
    let provider = Provider::new(true).await;
    assert_eq!(
        f.service_a
            .generate_agent_title(&f.id, Some(&context()), Some(&provider.generator))
            .await,
        Err(AgentTitleServiceError::EffectUnknown)
    );
    insert_provenance(&f).await;
    // Within the claim deadline a result could still be accepted.
    assert!(!purge_allowed(&f).await);
    // Move the retained capability window into the past, keeping its length.
    f.a.agents
        .update_one(
            doc! {"_id": f.id.as_str()},
            doc! {"$set": {
                "title_effect_custody.intent.acquired_ms": 1_000_i64,
                "title_effect_custody.intent.expires_ms": 46_000_i64,
            }},
        )
        .await
        .unwrap();
    assert!(f.b.agent_title_effect(&f.id).await.unwrap().unknown());
    assert!(purge_allowed(&f).await);
    // Permanent identity is still required.
    f.a.agent_creation_provenance
        .delete_one(doc! {"_id": f.id.as_str()})
        .await
        .unwrap();
    assert!(!purge_allowed(&f).await);
    assert_eq!(provider.calls(), 1);
    f.cleanup().await;
}

async fn purge_allowed(f: &Fixture) -> bool {
    let mut session = f.client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let row =
        f.a.agents
            .find_one(doc! {"_id": f.id.as_str()})
            .session(&mut session)
            .await
            .unwrap()
            .unwrap();
    let allowed =
        crate::store::agent_title_effect::purge::require_finality(&f.a, &[row], &mut session)
            .await
            .is_ok();
    session.abort_transaction().await.unwrap();
    allowed
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_wrong_identity_and_oversized_envelopes_are_refused() {
    let f = Fixture::new("invalid_envelope").await;
    let (claim, intent) = f.intent().await;
    assert!(f.a.begin_agent_title_effect(&claim, &intent).await.unwrap());
    let original = f
        .row()
        .await
        .get_document("title_effect_custody")
        .unwrap()
        .clone();
    let mut wrong_agent = original.clone();
    wrong_agent
        .get_document_mut("intent")
        .unwrap()
        .insert("agent_id", "agt_other");
    let mut oversized = original.clone();
    oversized
        .get_document_mut("intent")
        .unwrap()
        .insert("strategy", "x".repeat(4096));
    let mut unknown_field = original.clone();
    unknown_field.insert("new_dispatch_permission", true);
    for envelope in [wrong_agent, oversized, unknown_field] {
        f.a.agents
            .update_one(
                doc! {"_id": f.id.as_str()},
                doc! {"$set": {"title_effect_custody": envelope}},
            )
            .await
            .unwrap();
        assert!(matches!(
            f.b.agent_title_effect(&f.id).await,
            Err(AgentTitleStoreError::Corrupt)
        ));
        assert!(!purge_allowed(&f).await);
    }
    f.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn title_custody_activation_does_not_redispatch_a_legacy_unknown_attempt() {
    let f = Fixture::new("legacy_unknown").await;
    let provider = Provider::new(true).await;
    let off = Arc::new(peer(&f.name, false).await);
    let old = AgentTitleService::new(off, "before-activation").unwrap();
    assert_eq!(
        old.generate_agent_title(&f.id, Some(&context()), Some(&provider.generator))
            .await,
        Err(AgentTitleServiceError::GenerationUnavailable)
    );
    assert_eq!(provider.calls(), 1);
    expire(&f.a, &f.id).await;
    assert_eq!(
        f.service_a
            .generate_agent_title(&f.id, Some(&context()), Some(&provider.generator))
            .await,
        Err(AgentTitleServiceError::EffectUnknown)
    );
    assert!(f.b.agent_title_effect(&f.id).await.unwrap().unknown());
    assert!(peer(&f.name, false)
        .await
        .agent_title_effect(&f.id)
        .await
        .unwrap()
        .unknown());
    assert!(!purge_allowed(&f).await);
    assert_eq!(provider.calls(), 1);
    f.cleanup().await;
}

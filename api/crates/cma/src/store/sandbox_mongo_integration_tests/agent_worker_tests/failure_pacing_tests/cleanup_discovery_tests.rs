use super::ownership_retry_wake_tests::fixture;
use super::*;
use crate::store::agent_failure_pacing::discovery;
use futures_util::TryStreamExt;
use layer2_core::sandbox::store::{AgentWorkDiscovery, PendingAgentWork, StartupWorkFilter};
use layer2_core::sandbox::{AgentStatus, AgentStopReason};
use std::collections::{BTreeMap, BTreeSet};

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn dense_agent_discovery_streams_the_first_page_before_joining_all_parents() {
    let (client, name, store, template, _, _) = fixture("dense_hold_discovery").await;
    let store = store.with_ownership_retry_hints(false);
    let mut rows = Vec::new();
    for index in 0..1_024 {
        let mut agent = template.clone();
        let id = format!("000-due-{index:04}");
        agent.id = AgentId(id.clone());
        agent.creation_key = id.clone();
        agent.worktree_id = WorktreeId(format!("{id}-tree"));
        agent.work.lease = None;
        rows.push(crate::store::agents::agent_document(&agent).unwrap());
    }
    store.agents.insert_many(rows).await.unwrap();
    let now = layer2_core::rfc3339_at(SystemTime::now());
    let exclusions = BTreeSet::new();
    let filter = AgentWorkDiscovery::new(&exclusions, StartupWorkFilter::All);
    let database = client.database(&name);
    for (variant, pipeline) in [
        (
            "local",
            discovery::filtered_pipeline(&store, &now, 1, filter),
        ),
        (
            "owned",
            discovery::filtered_owned_pipeline(&store, &now, 1, None, filter),
        ),
    ] {
        let explained = database
            .run_command(doc! {
                "explain": {"aggregate": store.agents.name(), "pipeline": pipeline, "cursor": {}},
                "verbosity": "executionStats",
            })
            .await
            .unwrap();
        let stages = explained.get_array("stages").unwrap();
        let cursor = stages[0]
            .as_document()
            .unwrap()
            .get_document("$cursor")
            .unwrap();
        let docs_examined = |stats: &Document| {
            stats
                .get_i64("totalDocsExamined")
                .or_else(|_| stats.get_i32("totalDocsExamined").map(i64::from))
                .unwrap()
        };
        let agents_examined = docs_examined(cursor.get_document("executionStats").unwrap());
        let lookup = stages
            .iter()
            .filter_map(|stage| stage.as_document())
            .find(|stage| {
                stage
                    .get_document("$lookup")
                    .is_ok_and(|lookup| lookup.get_str("as").ok() == Some("maintenance_parent"))
            })
            .unwrap();
        let parents_examined = docs_examined(lookup);
        println!(
            "{variant} discovery examined {agents_examined} Agents and {parents_examined} parents"
        );
        assert!((1..=128).contains(&agents_examined));
        assert_eq!(parents_examined, 1);
    }
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn ledger_maintenance_filters_both_agent_discovery_arms_before_paging() {
    use crate::store::sandbox_mongo_integration_tests::sandbox;
    use crate::store::work_claim_authority::LEDGER_MAINTENANCE_FIELD;
    use mongodb::bson::Bson;

    let (client, name, store, template, _, _) = fixture("held_agent_discovery").await;
    let mut sibling = sandbox("sbx_unheld_agent_discovery");
    sibling.name = "unheld agent discovery".into();
    store.create_sandbox(sibling.clone()).await.unwrap();
    let candidate = |id: &str| {
        let mut agent = template.clone();
        agent.id = AgentId(id.into());
        agent.creation_key = id.into();
        agent.worktree_id = WorktreeId(format!("{id}-tree"));
        agent.work.lease = None;
        agent
    };
    let ordinary = candidate("000-held-ordinary");
    let paced = candidate("000-held-paced");
    let mut healthy = candidate("001-healthy");
    healthy.sandbox_id = sibling.id;
    let mut paced_row = crate::store::agents::agent_document(&paced).unwrap();
    let now = SystemTime::now();
    paced_row.insert(
        FIELD,
        doc! {
            "class": CLASS, "agent_revision": paced.revision as i64,
            "command_admission_generation": 0_i64, "attempts": paced.work.attempts, "repeats": 0,
            "sandbox_revision": 0_i64, "workspace_revision": 0_i64, "worktree_revision": 0_i64,
            "ready_at": &paced.work.ready_at,
            "until": layer2_core::rfc3339_at(now + Duration::from_secs(120)),
        },
    );
    store
        .agents
        .insert_many([
            crate::store::agents::agent_document(&ordinary).unwrap(),
            paced_row,
            crate::store::agents::agent_document(&healthy).unwrap(),
        ])
        .await
        .unwrap();

    let now = layer2_core::rfc3339_at(now);
    let exclusions = BTreeSet::new();
    let filter = AgentWorkDiscovery::new(&exclusions, StartupWorkFilter::All);
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let independent = Client::with_uri_str(uri).await.unwrap();
    let parents = independent
        .database(&name)
        .collection::<Document>(store.sandboxes.name());
    let parent_filter = doc! {"_id": ordinary.sandbox_id.as_str()};
    for hold in [
        None,
        Some(Bson::Document(doc! {"phase": "requested"})),
        Some(Bson::Null),
    ] {
        let limit = if let Some(hold) = hold {
            let updated = parents
                .update_one(
                    parent_filter.clone(),
                    doc! {"$set": {LEDGER_MAINTENANCE_FIELD: hold}},
                )
                .await
                .unwrap();
            assert_eq!(updated.matched_count, 1);
            1
        } else {
            3
        };
        let expected = if limit == 1 {
            vec![healthy.id.as_str()]
        } else {
            vec![ordinary.id.as_str(), paced.id.as_str(), healthy.id.as_str()]
        };
        for pipeline in [
            discovery::filtered_pipeline(&store, &now, limit, filter),
            discovery::filtered_owned_pipeline(&store, &now, limit, None, filter),
        ] {
            let rows: Vec<Document> = store
                .agents
                .aggregate(pipeline)
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            let ids: Vec<_> = rows.iter().map(|row| row.get_str("_id").unwrap()).collect();
            assert_eq!(ids, expected);
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn cleanup_and_acquisition_holds_preserve_healthy_work_and_controls_behind_a_full_page() {
    let (client, name, store, template, _authority, _) = fixture("cleanup_discovery").await;
    let now = layer2_core::rfc3339_at(SystemTime::now());
    let candidate = |id: String| {
        let mut agent = template.clone();
        agent.id = AgentId(id.clone());
        agent.creation_key = id.clone();
        agent.worktree_id = WorktreeId(format!("{id}-tree"));
        agent.work.lease = None;
        agent
    };
    let mut held = BTreeSet::new();
    let mut cooldowns = BTreeMap::new();
    let mut rows = Vec::new();
    let until = layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(120));
    for index in 0..200 {
        let agent = candidate(format!("000-held-{index:03}"));
        held.insert(agent.id.clone());
        cooldowns.insert(
            agent.id.clone(),
            PendingAgentWork::from_agent(&agent).unwrap(),
        );
        let mut row = crate::store::agents::agent_document(&agent).unwrap();
        // Stale parent revisions make these due through the wakeup arm of the union.
        if index % 2 == 0 {
            row.insert(FIELD, doc! {"class": CLASS, "agent_revision": agent.revision as i64,
                "command_admission_generation": 0_i64, "attempts": agent.work.attempts, "repeats": 0,
                "sandbox_revision": 0_i64, "workspace_revision": 0_i64, "worktree_revision": 0_i64,
                "ready_at": &agent.work.ready_at, "until": &until});
        }
        rows.push(row);
    }
    let healthy = candidate("001-healthy".into());
    let mut stop = candidate("002-stop".into());
    cooldowns.insert(
        stop.id.clone(),
        PendingAgentWork::from_agent(&stop).unwrap(),
    );
    stop.request_stop(AgentStopReason::Owner, now.clone())
        .unwrap();
    let mut delete = candidate("003-delete".into());
    delete.status = AgentStatus::Stopped;
    // Snapshot a pending predecessor on the same ID. Deletion changes the work.
    cooldowns.insert(
        delete.id.clone(),
        PendingAgentWork::from_agent(&delete).unwrap(),
    );
    delete.request_delete(now.clone()).unwrap();
    held.extend([stop.id.clone(), delete.id.clone()]);
    for agent in [&healthy, &stop, &delete] {
        rows.push(crate::store::agents::agent_document(agent).unwrap());
    }
    store.agents.insert_many(rows).await.unwrap();
    assert_eq!(
        store
            .due_agent_work_with_cleanup_holds(&now, 1, &held)
            .await
            .unwrap(),
        vec![healthy.id.clone()]
    );
    assert_eq!(
        store
            .due_agent_work_with_cleanup_holds(&now, 3, &held)
            .await
            .unwrap(),
        vec![healthy.id.clone(), stop.id.clone(), delete.id.clone()]
    );
    assert_eq!(
        store.due_agent_work(&now, 1).await.unwrap(),
        vec![AgentId("000-held-000".into())]
    );
    let no_receiver_holds = BTreeSet::new();
    let filter = AgentWorkDiscovery {
        receiver_holds: &no_receiver_holds,
        startup: StartupWorkFilter::All,
        acquisition_cooldowns: Some(&cooldowns),
    };
    assert_eq!(
        store
            .due_agent_work_filtered(&now, 3, filter)
            .await
            .unwrap(),
        vec![healthy.id, stop.id, delete.id]
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo replica set via CHRONO_TEST_MONGODB_URI"]
async fn startup_sibling_hold_filters_both_discovery_arms_and_keeps_a_directed_probe() {
    let (client, name, store, template, _authority, _) = fixture("startup_sibling_discovery").await;
    let now = layer2_core::rfc3339_at(SystemTime::now());
    let candidate = |id: &str| {
        let mut agent = template.clone();
        agent.id = AgentId(id.into());
        agent.creation_key = id.into();
        agent.worktree_id = WorktreeId(format!("{id}-tree"));
        agent.work.lease = None;
        agent
    };
    let ordinary = candidate("000-held-ordinary");
    let paced = candidate("000-held-paced");
    let mut paced_row = crate::store::agents::agent_document(&paced).unwrap();
    paced_row.insert(
        FIELD,
        doc! {
            "class": CLASS, "agent_revision": paced.revision as i64,
            "command_admission_generation": 0_i64, "attempts": paced.work.attempts, "repeats": 0,
            "sandbox_revision": 0_i64, "workspace_revision": 0_i64, "worktree_revision": 0_i64,
            "ready_at": &paced.work.ready_at,
            "until": layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(120)),
        },
    );
    let mut healthy = candidate("001-healthy");
    healthy.sandbox_id = layer2_core::SandboxId("healthy-sandbox".into());
    let mut stop = candidate("002-stop");
    stop.request_stop(AgentStopReason::Owner, now.clone())
        .unwrap();
    let mut delete = candidate("003-delete");
    delete.status = AgentStatus::Stopped;
    delete.request_delete(now.clone()).unwrap();
    let mut rows = vec![paced_row];
    for agent in [&ordinary, &healthy, &stop, &delete] {
        rows.push(crate::store::agents::agent_document(agent).unwrap());
    }
    store.agents.insert_many(rows).await.unwrap();
    let held = BTreeSet::from([ordinary.sandbox_id.clone()]);
    let cleanup = BTreeSet::new();
    for (scope, limit, expected) in [
        (
            StartupWorkFilter::Exclude(&held),
            1,
            vec![healthy.id.as_str()],
        ),
        (
            StartupWorkFilter::Exclude(&held),
            3,
            vec![healthy.id.as_str(), stop.id.as_str(), delete.id.as_str()],
        ),
        (
            StartupWorkFilter::Probe(&ordinary.sandbox_id),
            1,
            vec![ordinary.id.as_str()],
        ),
        (
            StartupWorkFilter::Probe(&ordinary.sandbox_id),
            3,
            vec![ordinary.id.as_str(), paced.id.as_str()],
        ),
    ] {
        let filter = AgentWorkDiscovery::new(&cleanup, scope);
        for pipeline in [
            discovery::filtered_pipeline(&store, &now, limit, filter),
            discovery::filtered_owned_pipeline(&store, &now, limit, None, filter),
        ] {
            let rows: Vec<Document> = store
                .agents
                .aggregate(pipeline)
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap();
            let ids: Vec<_> = rows.iter().map(|row| row.get_str("_id").unwrap()).collect();
            assert_eq!(ids, expected);
        }
    }
    client.database(&name).drop().await.unwrap();
}

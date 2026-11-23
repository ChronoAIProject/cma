use super::*;
use axum::http::StatusCode;
use layer2_core::sandbox::{
    channel_usage::*,
    store::{AgentChannelUsageStore, AgentProvenanceInventoryStore},
};
use std::{sync::atomic::AtomicBool, time::Duration};

const PATH: &str = "/api/v1/agents/provenance";
const ID: &str = "agt_list_0000";

async fn attach(f: &fixture::Fixture) -> Document {
    provenance::ready(&f.store, 0, false).await;
    let before = f
        .store
        .agent_channel_usage_snapshot(&AgentId(ID.into()))
        .await
        .unwrap();
    f.store
        .record_agent_channel_usage(ChannelUsageMutation {
            policy_fence: before.provenance.resource.fence(),
            creation: before.state.creation,
            expected_revision: 0,
            key_sha256: layer2_core::profiles::content_digest(&"latest-event-attach").unwrap(),
            actor: ResourceActor::Human {
                subject: identity::owner(0),
            },
            change: ChannelUsageChange::Attach {
                binding: ChannelBindingReference {
                    binding_id: "latest-event-binding".into(),
                    bridge_id: "latest-event-bridge".into(),
                    partition_id: "latest-event-partition".into(),
                    generation: 1,
                    previous_binding_id: None,
                },
            },
            reason: ChannelBindingChangeReason::Attachment,
            at: MINUTE.into(),
        })
        .await
        .unwrap();
    f.store
        .channel_usage_events
        .find_one(doc! {"agent_id":ID})
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_provenance_latest_event_preserves_array_equality_and_orphan_refusal() {
    let f = fixture::Fixture::new(2, true).await;
    let latest = attach(&f).await;
    let old = f.app(false, false);
    let new = f.app(true, false);
    let expected = public::request(&old, PATH).await;
    assert_eq!(expected.0, StatusCode::OK);
    assert_eq!(public::request(&new, PATH).await, expected);

    let mut malformed = latest.clone();
    malformed.insert("_id", "older-array-event");
    malformed.insert("agent_id", vec![ID, "unselected-agent"]);
    malformed.insert("revision", 0_i64);
    malformed.remove("binding_id");
    f.store
        .channel_usage_events
        .insert_one(malformed)
        .await
        .unwrap();
    assert_eq!(public::request(&old, PATH).await, expected);
    assert_eq!(public::request(&new, PATH).await, expected);

    f.store
        .channel_usage_events
        .update_one(
            doc! {"_id":"older-array-event"},
            doc! {"$set":{"revision":2_i64}},
        )
        .await
        .unwrap();
    let corrupt = public::request(&old, PATH).await;
    assert!(!corrupt.0.is_success());
    assert_eq!(public::request(&new, PATH).await, corrupt);

    f.store
        .channel_usage_events
        .delete_one(doc! {"_id":"older-array-event"})
        .await
        .unwrap();
    assert_eq!(public::request(&new, PATH).await, expected);
    f.store
        .agent_channel_usage
        .delete_one(doc! {"_id":ID})
        .await
        .unwrap();
    let orphan = public::request(&old, PATH).await;
    assert!(!orphan.0.is_success());
    assert_eq!(public::request(&new, PATH).await, orphan);
    f.close().await;
}

#[derive(Default)]
struct Pause {
    armed: AtomicBool,
    entered: tokio::sync::Notify,
    resume: (Mutex<bool>, std::sync::Condvar),
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn personal_provenance_selected_agents_and_latest_event_share_one_snapshot() {
    let f = fixture::Fixture::new(1, true).await;
    let mut later = attach(&f).await;
    let scope = ResourceOwner::Person {
        subject: identity::owner(0),
    };
    let query = AgentListQuery::default();
    let page = PageRequest::new(20, None).unwrap();
    let expected = f
        .store
        .scan_agent_provenance_inventory(&scope, &query, &page)
        .await
        .unwrap();
    assert_eq!(expected.len(), 1);
    let pause = Arc::new(Pause::default());
    let observed = pause.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command.get_str("comment") == Ok("cma.personal_agent_evidence")
                && observed.armed.swap(false, Ordering::SeqCst)
            {
                observed.entered.notify_one();
                let (lock, wake) = &observed.resume;
                let (_ready, timeout) = wake
                    .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(5), |ready| {
                        !*ready
                    })
                    .unwrap();
                assert!(!timeout.timed_out(), "writer must release the capture");
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &f.name)
        .await
        .unwrap()
        .with_resource_list_batch_reads(true);
    pause.armed.store(true, Ordering::SeqCst);
    let task = tokio::spawn(async move {
        store
            .scan_agent_provenance_inventory(&scope, &query, &page)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), pause.entered.notified())
        .await
        .unwrap();
    let original = f
        .store
        .agents
        .find_one(doc! {"_id":ID})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        f.store
            .agents
            .delete_one(doc! {"_id":ID})
            .await
            .unwrap()
            .deleted_count,
        1
    );
    later.insert("_id", "later-invalid-event");
    later.insert("revision", 2_i64);
    later.remove("binding_id");
    f.store
        .channel_usage_events
        .insert_one(later)
        .await
        .unwrap();
    *pause.resume.0.lock().unwrap() = true;
    pause.resume.1.notify_one();
    assert_eq!(task.await.unwrap().unwrap(), expected);
    let scope = ResourceOwner::Person {
        subject: identity::owner(0),
    };
    assert!(f
        .store
        .scan_agent_provenance_inventory(
            &scope,
            &AgentListQuery::default(),
            &PageRequest::new(20, None).unwrap()
        )
        .await
        .unwrap()
        .is_empty());
    f.store.agents.insert_one(original).await.unwrap();
    assert!(matches!(
        f.store
            .scan_agent_provenance_inventory(
                &scope,
                &AgentListQuery::default(),
                &PageRequest::new(20, None).unwrap()
            )
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    f.close().await;
    client.shutdown().await;
}

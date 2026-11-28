//! Full native authority races on the production due reserved binding path.
use super::*;
use serde_json::json;
use std::sync::atomic::AtomicUsize;

#[path = "binding_authority_api.rs"]
mod api;
#[path = "failed_child_retirement_tests.rs"]
mod failed_child_retirement;
#[path = "binding_authority_fixture.rs"]
mod fixture;
#[path = "binding_authority_transport.rs"]
mod transport;

#[path = "../reconciliation_tests/startup_native_order.rs"]
mod startup_native_order;

#[tokio::test]
async fn due_reserved_native_status_rechecks_every_authority_and_protects_cleanup() {
    for fresh in [false, true] {
        for boundary in [false, true] {
            for change in [
                "none",
                "claim_uid",
                "assignment",
                "pod",
                "container",
                "pvc",
                "image",
                "generation",
            ] {
                let (original, mut active, now) = active_fixture().await;
                let native = fixture::NativeFixture::new(&mut active).await;
                let store = Arc::new(InMemorySandboxStore::default());
                active.revision = 1; // The independent store starts from this native record.
                store.create_sandbox(active.clone()).await.unwrap();
                let mut agent = lifecycle_queue_tests::ready_agent(&active, now);
                agent.conversation.runtime_generation = Some(1);
                store.create_agent(agent.clone()).await.unwrap();
                let reconciler = SandboxReconciler::new(
                    store.clone(),
                    native.adapter.clone(),
                    Arc::new(original.harness.policy.clone()),
                    "test".into(),
                );
                let first = reconciler.run_due(now);
                tokio::pin!(first);
                tokio::select! {
                    _ = native.transport.entered.notified() => {},
                    result = &mut first => panic!("status must enter the first binding attempt: {result:?}"),
                }
                // Advance only while status is deliberately stalled. Kubernetes
                // I/O runs with the real clock, so no socket race uses a sleep.
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(5)).await;
                tokio::time::resume();
                assert!(first.await.unwrap());
                assert!(reconciler.binding_is_held(&active.id));
                tokio::time::pause();
                tokio::time::advance(Duration::from_secs(30)).await;
                tokio::time::resume();
                let assignments = native.api.state.lock().unwrap().assignments.len();
                let calls = native.api.state.lock().unwrap().calls.len();
                if fresh {
                    native
                        .api
                        .state
                        .lock()
                        .unwrap()
                        .rows
                        .get_mut(api::POD)
                        .unwrap()["metadata"]["uid"] = json!("fresh-process");
                }
                let (path, pointer, value) = native.mutation(change);
                *native.transport.fault.lock().unwrap() = Some((boundary, path, pointer, value));
                // Fresh inventory consumes the discovery slice. The reserved
                // attempt is dispatched by the next invocation, on this cursor.
                assert!(!reconciler.run_due(now).await.unwrap());
                assert!(!native.transport.fired.load(Ordering::SeqCst));
                assert_eq!(native.transport.responses.load(Ordering::SeqCst), 0);
                assert!(
                    !tokio::time::timeout(Duration::from_secs(5), reconciler.run_due(now))
                        .await
                        .expect("a reserved deadline must not count as authority refusal")
                        .unwrap()
                );
                assert!(
                    native.transport.fired.load(Ordering::SeqCst),
                    "{change}, after={boundary}, fresh={fresh}"
                );
                assert_eq!(native.transport.responses.load(Ordering::SeqCst), 1);
                assert_eq!(
                    reconciler.binding_is_held(&active.id),
                    change != "none",
                    "{change}, after={boundary}, fresh={fresh}"
                );
                let expected = native.api.state.lock().unwrap().rows.clone();
                // These changes also invalidate cleanup inventory itself. Its
                // exact refusal must retain protection, never grant authority.
                let cleanup_refusal = match change {
                    "claim_uid" | "assignment" => {
                        Some("runtime rejected request: native home belongs to another claim")
                    }
                    "pvc" => Some(
                        "runtime rejected request: native PVC differs from its admitted storage and controller authority",
                    ),
                    "image" => Some("Sandbox spec differs from its admitted configuration"),
                    _ => None,
                };
                let mut completed = false;
                let mut refused = false;
                for _ in 0..4 {
                    match reconciler.run_due(now).await {
                        Ok(true) => {
                            completed = true;
                            break;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            let detail =
                                cleanup_refusal.expect("cleanup must complete for this authority");
                            assert!(
                                matches!(error, SandboxStoreError::Unavailable(ref message)
                                if message == &format!("sandbox runtime authority is unavailable: {detail}")),
                                "{change}, after={boundary}, fresh={fresh}: {error:?}"
                            );
                            refused = true;
                            break;
                        }
                    }
                }
                assert_eq!(refused, cleanup_refusal.is_some());
                assert_eq!(completed, cleanup_refusal.is_none());
                assert_eq!(
                    store.get_sandbox(&active.id).await.unwrap(),
                    Some(active.clone())
                );
                assert_eq!(store.get_agent(&agent.id).await.unwrap(), Some(agent));
                assert_eq!(reconciler.binding_is_held(&active.id), change != "none");
                let state = native.api.state.lock().unwrap();
                assert_eq!(state.assignments.len(), assignments + usize::from(fresh));
                assert_eq!(state.rows, expected);
                assert!(state.calls.iter().all(|(method, _)| method != "DELETE"));
                assert!(state.calls[calls..]
                    .iter()
                    .all(|(method, _)| method == "GET"));
                for path in [&native.claim_path, api::MEMBER, api::POD, api::VOLUME] {
                    assert!(state.rows.contains_key(path));
                }
            }
        }
    }
}

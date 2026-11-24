use super::super::super::reconciliation::EffectReconciler;
use super::super::process_fixture::*;
use super::super::reconciliation::fixture::Provider;
use super::*;
use layer2_core::resource_access::revocations::{
    ResourceRevocationService, ResourceRevocations, RevocationStage, RevocationStatus,
};
use mongodb::bson::doc;
use std::{path::PathBuf, time::Duration};
use tokio::process::Child;

async fn reached(fixture: &Fixture, child: &mut Child) -> Script {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "worker exited before its cut"
            );
            let script = fixture
                .database()
                .collection::<Script>(COLLECTION)
                .find_one(doc! {"_id": "participant"})
                .await
                .unwrap()
                .unwrap();
            if script.reached {
                return script;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

async fn status(fixture: &Fixture, operation: &RevocationOperationV1) -> RevocationStatus {
    ResourceRevocationService::new(fixture.access.clone(), fixture.reopened().await)
        .await
        .unwrap()
        .status(
            &fixture::context("owner"),
            None,
            &fixture.selector,
            operation.key.operation_id.as_str(),
        )
        .await
        .unwrap()
}

async fn retained_effect(fixture: &Fixture, expected: &EffectRecordV1) {
    let effects = fixture
        .reopened()
        .await
        .effects_for(
            &expected.permission.intent.registration,
            0,
            MAX_PAGE_MEMBERS,
        )
        .await
        .unwrap();
    assert_eq!(effects.as_slice(), std::slice::from_ref(expected));
}

// The default is a same-version process contract. An explicit, externally
// hash-bound test artifact also exercises an older worker against current rows.
// Receiver receipts and provider replies remain synthetic in either mode.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn worker_replacement_preserves_unresolved_effects_through_finalization() {
    let replacement = match std::env::var_os("CMA_REVOCATION_COMPATIBILITY_BINARY") {
        Some(path) => {
            println!("replacement mode: explicitly supplied test artifact");
            PathBuf::from(path)
        }
        None => {
            println!("replacement mode: current test binary");
            std::env::current_exe().unwrap()
        }
    };
    assert!(replacement.is_file());
    for cut in [Cut::BeforeStop, Cut::AfterReceipt] {
        for outcome in [
            wire::EffectOutcome::CommitConfirmed,
            wire::EffectOutcome::Rejected,
        ] {
            let fixture = Fixture::new().await;
            let effect = effects::prepare(&fixture).await;
            let registration = &effect.permission.intent.registration;
            let row = fixture.store.record(registration).await.unwrap().unwrap();
            let original = fixture.freeze().await;
            let scripts = fixture.database().collection::<Script>(COLLECTION);
            let mut script = Script::new(&row, &original, cut);
            script.receipt.evidence = wire::StopEvidenceKind::CooperativeQuiescence;
            script.receipt.validate_for(&script.request).unwrap();
            scripts.insert_one(&script).await.unwrap();
            let pending = status(&fixture, &original).await;
            assert_eq!(pending.stage, RevocationStage::Pending);
            assert_eq!(pending.unresolved_effects, 1);
            assert!(pending.receipt.is_none());

            let mut current = process::spawn(&fixture, true);
            let first_pid = current.id().unwrap();
            let before = reached(&fixture, &mut current).await;
            current.kill().await.unwrap();
            let killed = current.wait_with_output().await.unwrap();
            assert!(!killed.status.success());
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                assert_eq!(killed.status.signal(), Some(9));
            }
            assert!(!String::from_utf8(killed.stdout)
                .unwrap()
                .contains(COMPLETED));
            assert_eq!(before.retained, matches!(cut, Cut::AfterReceipt));
            assert_eq!(before.stop_effects, u32::from(before.retained));
            assert!(
                fixture
                    .store
                    .revocation(&original.key.operation_id)
                    .await
                    .unwrap()
                    .unwrap()
                    == original
            );
            let stopped = fixture.store.record(registration).await.unwrap().unwrap();
            assert_eq!(stopped.barrier, Some(script.receipt.barrier.clone()));
            assert!(stopped.completion.is_none());
            retained_effect(&fixture, &effect).await;

            let reader = process::spawn_binary(&fixture, false, &replacement);
            let replacement_pid = reader.id().unwrap();
            assert_ne!(replacement_pid, std::process::id());
            let output = tokio::time::timeout(Duration::from_secs(15), reader.wait_with_output())
                .await
                .unwrap()
                .unwrap();
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(output.status.success(), "{stdout}");
            assert!(
                stdout.contains(COMPLETED) && stdout.contains("1 passed"),
                "{stdout}"
            );
            let finalized = final_operation(&fixture, &original).await;
            assert!(fixture.freeze().await == finalized);
            retained_effect(&fixture, &effect).await;
            let mut expected = row.clone();
            expected.barrier = Some(script.receipt.barrier.clone());
            expected.completion = Some(MemberCompletionV1::Execution {
                receipt: script.receipt.clone(),
            });
            assert!(fixture.store.record(registration).await.unwrap().unwrap() == expected);
            let after = scripts
                .find_one(doc! {"_id": "participant"})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(after.stop_effects, 1);
            let calls = match cut {
                Cut::BeforeStop => vec!["query", "barrier", "stop", "query", "barrier", "stop"],
                Cut::AfterReceipt => vec!["query", "barrier", "stop", "query"],
            };
            assert_eq!(after.calls, calls);

            let effective = status(&fixture, &original).await;
            assert_eq!(effective.stage, RevocationStage::Effective);
            assert_eq!(effective.operation_id, pending.operation_id);
            assert_eq!(effective.deadline_unix_ms, pending.deadline_unix_ms);
            assert_eq!(
                (
                    effective.participants,
                    effective.stopped,
                    effective.unresolved_effects
                ),
                (1, 1, 1)
            );
            assert!(effective.receipt.is_some());
            let worker = RevocationWorker::new(fixture.reopened().await, Directory::new(&[]));
            assert_eq!(
                worker.reconcile(&original.key.operation_id).await.unwrap(),
                Progress::Effective
            );
            assert_eq!(
                scripts
                    .find_one(doc! {"_id": "participant"})
                    .await
                    .unwrap()
                    .unwrap()
                    .calls,
                calls
            );
            let mut forbidden = effect.permission.intent.clone();
            forbidden.effect_id = id("fresh-effect-after-finalization");
            assert!(matches!(
                fixture
                    .coordinator
                    .prepare_effect(&forbidden.execution.receiver, &forbidden,)
                    .await,
                Err(ExecutionError::Closing { .. }) | Err(ExecutionError::AuthorityChanged)
            ));
            retained_effect(&fixture, &effect).await;

            let provider = Provider::new(&effect, outcome);
            let terminal = provider.reply.lock().unwrap().clone();
            *provider.reply.lock().unwrap() = Ok(None);
            let reconciler = EffectReconciler::new(fixture.reopened().await, provider.clone());
            let unknown = reconciler.reconcile(registration).await.unwrap();
            assert_eq!((unknown.terminal, unknown.unresolved), (0, 1));
            assert_eq!(
                unknown.failures,
                vec![(
                    effect.permission.intent.effect_id.clone(),
                    ExecutionError::Unknown
                )]
            );
            retained_effect(&fixture, &effect).await;
            *provider.reply.lock().unwrap() = terminal;
            assert_eq!(
                reconciler.reconcile(registration).await.unwrap().terminal,
                1
            );
            assert_eq!(
                reconciler.reconcile(registration).await.unwrap().terminal,
                1
            );
            assert_eq!(
                *provider.calls.lock().unwrap(),
                vec![effect.permission.intent.clone(); 2]
            );
            let retained = fixture
                .store
                .effects_for(registration, 0, MAX_PAGE_MEMBERS)
                .await
                .unwrap();
            assert_eq!(retained.len(), 1);
            assert_eq!(retained[0].permission, effect.permission);
            assert_eq!(retained[0].observations.len(), 2);
            assert_eq!(retained[0].observations[0], effect.observations[0]);
            assert_eq!(retained[0].observations[1].outcome, outcome);
            assert!(
                fixture
                    .store
                    .revocation(&original.key.operation_id)
                    .await
                    .unwrap()
                    .unwrap()
                    == finalized
            );
            let resolved = status(&fixture, &original).await;
            assert_eq!(resolved.unresolved_effects, 0);
            assert!(resolved.receipt == effective.receipt);
            println!("{cut:?}/{outcome:?}: killed {first_pid}; replacement {replacement_pid}; original effect and receipt retained");
            fixture.cleanup().await;
        }
    }
}

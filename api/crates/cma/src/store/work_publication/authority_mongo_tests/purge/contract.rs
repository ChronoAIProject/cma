//! Purge retry authority, uncertainty and atomic cleanup against real Mongo.
use super::*;
use futures_util::FutureExt;
use layer2_core::sandbox::store::{WorkLeaseGrant, WORK_LEASE_SECONDS};
use mongodb::event::command::CommandEvent;
use std::sync::{Arc, Mutex};

#[path = "cases.rs"]
mod cases;
#[path = "control.rs"]
mod control;
#[path = "fixture.rs"]
mod fixture;
#[path = "observations.rs"]
mod observations;
#[path = "refusals.rs"]
mod refusals;
use fixture::{PurgeFixture, State};

#[derive(Clone, Copy)]
enum Case {
    Clean,
    Abort,
    Exhausted,
    NonTransient,
    Unknown { transient: bool, duplicate: bool },
    Commit { committed: bool },
    Latch { transient: bool },
    Late { persistent: bool },
    Cancelled,
    Expired,
    Successor,
}

async fn exercise(case: Case, scoped: bool) {
    let p = PurgeFixture::new().await;
    let expected_successor = Arc::new(Mutex::new(None));
    let start = if matches!(case, Case::Expired) {
        Instant::now() - Duration::from_secs(WORK_LEASE_SECONDS + 1)
    } else {
        Instant::now()
    };
    let authority = WorkAuthority::new(ClaimedWork::Sandbox(Box::new(p.claimed.clone())), start);
    if matches!(case, Case::Cancelled) {
        let pending = authority.run(
            std::future::pending::<Result<(), SandboxStoreError>>(),
            std::future::pending::<Result<Option<WorkLeaseGrant>, SandboxStoreError>>,
        );
        assert!(pending.now_or_never().is_none());
    }
    let before = p.state().await;
    p.f.reset_trace();
    let result = authority
        .run(
            async {
                configure(&p, case, expected_successor.clone()).await;
                let work =
                    p.f.store
                        .purge_sandbox(&p.claimed.id, p.claimed.revision, MINUTE);
                if scoped {
                    crate::store::work_publication::scope(&p.f.store, work).await
                } else {
                    work.await
                }
            },
            || async {
                p.f.independent
                    .renew_sandbox_authority(
                        &p.claimed,
                        &now(),
                        &p.claimed.work.lease.as_ref().unwrap().expires_at,
                    )
                    .await
            },
        )
        .await;
    *p.f.observer.lock().unwrap() = None;
    p.f.clear().await;
    let starts = p.f.starts();
    let events = p.f.events.lock().unwrap().clone();
    let after = p.state().await;
    p.f.drop_database().await;
    observations::report(&events);

    let (start_count, commit_count, success, known_hits) = match case {
        Case::Clean => (1, 1, true, 0),
        Case::Abort => (2, 1, true, 1),
        Case::Exhausted => (5, 0, false, 5),
        Case::Late { persistent: false } => (2, 1, true, 1),
        Case::Late { persistent: true } => (5, 0, false, 5),
        Case::Commit { committed } => (1, 5, committed, 0),
        Case::Latch { transient } => (1, 2, false, usize::from(transient)),
        Case::Cancelled | Case::Expired => (0, 0, false, 0),
        Case::Successor => (2, 0, false, 1),
        Case::NonTransient | Case::Unknown { .. } => (1, 0, false, 0),
    };
    assert_fresh_transactions(&starts, start_count);
    let commits = events
        .iter()
        .filter_map(|event| match event {
            CommandEvent::Started(event) if event.command_name == "commitTransaction" => {
                Some(&event.command)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(commits.len(), commit_count);
    for commit in &commits {
        let last = starts.last().unwrap();
        assert!(commit.get("lsid") == last.get("lsid"));
        assert_eq!(commit.get("txnNumber"), last.get("txnNumber"));
    }
    let actual_hits = events.iter().filter(|event| matches!(event,
        CommandEvent::Failed(event)
            if matches!(event.failure.kind.as_ref(), mongodb::error::ErrorKind::Command(e) if e.code == 112)
            && event.failure.contains_label(mongodb::error::TRANSIENT_TRANSACTION_ERROR)
            && !event.failure.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT)
    )).count();
    assert_eq!(actual_hits, known_hits);
    let fences = events
        .iter()
        .filter_map(|event| match event {
            CommandEvent::Started(event) if event.command_name == "update" => {
                event.command.get_array("updates").ok()
            }
            _ => None,
        })
        .flatten()
        .filter_map(|item| item.as_document())
        .filter(|update| {
            update
                .get_document("u")
                .ok()
                .and_then(|u| u.get_document("$inc").ok())
                .is_some_and(|inc| inc.contains_key("work_publication_fence"))
        })
        .map(|update| update.get_document("q").unwrap())
        .collect::<Vec<_>>();
    if let Some(first) = fences.first() {
        assert!(
            fences.iter().all(|fence| fence == first),
            "every attempt must keep the original authority predicate"
        );
    }
    if matches!(case, Case::Late { .. }) {
        assert!(
            events
                .iter()
                .any(|event| matches!(event, CommandEvent::Succeeded(event)
            if event.command_name == "delete" && event.reply.get_i32("n").ok() == Some(1))),
            "a child deletion must actually precede the injected late failure"
        );
    }
    if let Case::Unknown {
        transient,
        duplicate,
    } = case
    {
        assert_eq!(
            events
                .iter()
                .filter(|event| observations::update_failure(event)
                    == Some((if duplicate { 11000 } else { 1 }, true, transient)))
                .count(),
            1
        );
    }
    let outcome = match &result {
        Ok(()) => "ok",
        Err(SandboxStoreError::Conflict) => "conflict",
        Err(SandboxStoreError::Unavailable(message))
            if message.contains("commit remained unknown") =>
        {
            "commit_still_present"
        }
        Err(SandboxStoreError::Unavailable(_)) => "unavailable",
        Err(SandboxStoreError::Corrupt(_)) => "corrupt",
        _ => "other_refusal",
    };
    eprintln!("purge_observation outcome={outcome} target_removed={} hierarchy_unchanged={} counts_before={:?} counts_after={:?}",
        after == p.removed(&before), after == before,
        before.iter().map(Vec::len).collect::<Vec<_>>(),after.iter().map(Vec::len).collect::<Vec<_>>());
    if success {
        assert!(
            result.is_ok(),
            "confirmed purge or exact committed reconciliation must succeed"
        );
        assert!(
            after == p.removed(&before),
            "purge must remove only the complete target hierarchy"
        );
    } else {
        if matches!(case, Case::Cancelled | Case::Expired | Case::Successor) {
            assert!(matches!(result, Err(SandboxStoreError::Conflict)));
        } else {
            assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
        }
        let successor = expected_successor.lock().unwrap();
        if matches!(case, Case::Successor) {
            assert!(successor.is_some());
        }
        assert!(
            after == *successor.as_ref().unwrap_or(&before),
            "refusal must leave all original or successor state unchanged"
        );
    }
    eprintln!("purge_contract starts={start_count} commits={commit_count} known_abort_hits={actual_hits} success={success}");
}

async fn configure(p: &PurgeFixture, case: Case, successor: Arc<Mutex<Option<State>>>) {
    use control::fail;
    match case {
        Case::Abort | Case::Exhausted | Case::Successor => {
            fail(
                &p.f,
                "update",
                if matches!(case, Case::Exhausted) {
                    5
                } else {
                    1
                },
                112,
                &["TransientTransactionError"],
                false,
            )
            .await;
            if matches!(case, Case::Successor) {
                control::successor(p, successor);
            }
        }
        Case::NonTransient => fail(&p.f, "update", 1, 1, &[], false).await,
        Case::Unknown {
            transient,
            duplicate,
        } => {
            let mut labels = vec!["UnknownTransactionCommitResult"];
            if transient {
                labels.push("TransientTransactionError");
            }
            fail(
                &p.f,
                "update",
                1,
                if duplicate { 11000 } else { 1 },
                &labels,
                false,
            )
            .await;
        }
        Case::Commit { committed } => {
            fail(
                &p.f,
                "commitTransaction",
                5,
                1,
                &["UnknownTransactionCommitResult"],
                committed,
            )
            .await;
            if committed {
                control::committed_visibility(p);
            }
        }
        Case::Latch { transient } => {
            fail(
                &p.f,
                "commitTransaction",
                1,
                1,
                &["UnknownTransactionCommitResult"],
                false,
            )
            .await;
            control::latch(p, transient);
        }
        Case::Late { persistent } => control::late_abort(p, persistent),
        _ => {}
    }
}

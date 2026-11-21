use super::*;
use layer3_adapter::{RuntimeHandle, RuntimeStartupFailure, RuntimeStartupFailureClass};

const ADAPTER_DELAY: Duration = Duration::from_secs(15 * 60);

fn binding(process: &str) -> Binding {
    Binding {
        runtime_id: "fixture-runtime".into(),
        generation: 3,
        process: process.into(),
        status: SandboxStatus::Active,
        credential_delivered: true,
    }
}

#[tokio::test(start_paused = true)]
async fn due_probe_is_single_flight_and_cancellation_rearms_the_directed_wake() {
    let holds = StartupHolds::default();
    let sandbox = SandboxId("fixture".into());
    let one = AgentId("one".into());
    let two = AgentId("two".into());
    let binding = binding("initial");
    let started = Instant::now();
    let deadline = started + ADAPTER_DELAY;
    holds.observe_hold(&sandbox, binding.clone(), started, deadline, started);
    assert_eq!(holds.next_probe(), Some(deadline));
    assert!(holds.discovery(started, 1).probe.is_none());
    assert!(matches!(
        holds.admit(&sandbox, &one, Some(binding.clone()), None, started, true),
        Admission::Hold
    ));

    tokio::time::advance(ADAPTER_DELAY).await;
    let now = Instant::now();
    let page = holds.discovery(now, 1);
    assert_eq!(page.probe.as_ref(), Some(&sandbox));
    assert!(page.excluded.contains(&sandbox));
    let Admission::Probe(first) =
        holds.admit(&sandbox, &one, Some(binding.clone()), None, now, true)
    else {
        panic!("the due sibling must acquire the probe");
    };
    assert!(holds.next_probe().is_none());
    assert!(matches!(
        holds.admit(
            &sandbox,
            &one,
            Some(binding.clone()),
            Some(&first),
            now,
            false
        ),
        Admission::Proceed
    ));
    assert!(matches!(
        holds.admit(
            &sandbox,
            &two,
            Some(binding.clone()),
            Some(&first),
            now,
            true
        ),
        Admission::Hold
    ));

    drop(first);
    assert_eq!(holds.next_probe(), Some(now + PROBE_RESELECTION_DELAY));
    assert!(matches!(
        holds.admit(&sandbox, &two, Some(binding.clone()), None, now, true),
        Admission::Hold
    ));
    tokio::time::advance(PROBE_RESELECTION_DELAY).await;
    let now = Instant::now();
    assert!(matches!(
        holds.admit(&sandbox, &two, Some(binding), None, now, true),
        Admission::Probe(_)
    ));
}

#[tokio::test(start_paused = true)]
async fn verified_binding_replacement_and_serving_release_with_staggered_recovery() {
    for changed in [false, true] {
        let holds = StartupHolds::default();
        let id = SandboxId("fixture".into());
        let old = binding("initial");
        let started = Instant::now();
        holds.observe_hold(&id, old.clone(), started, started + ADAPTER_DELAY, started);
        tokio::time::advance(REFRESH_INTERVAL).await;
        let now = Instant::now();
        let refresh = holds.refresh_candidates(now).pop().unwrap();
        holds.refreshed(refresh, Some(old.clone()), now);
        assert!(holds.discovery(now, 1).excluded.contains(&id));
        // A concurrent success that started before the hold cannot release it.
        holds.observe_serving(&id, &old, started - Duration::from_secs(1), now);
        assert!(holds.discovery(now, 1).excluded.contains(&id));

        let current = if changed {
            tokio::time::advance(REFRESH_INTERVAL).await;
            let now = Instant::now();
            let replacement = binding("replacement");
            let refresh = holds.refresh_candidates(now).pop().unwrap();
            holds.refreshed(refresh, Some(replacement.clone()), now);
            replacement
        } else {
            holds.observe_serving(&id, &old, now, now);
            old.clone()
        };
        let now = Instant::now();
        assert!(holds.next_probe().is_none());
        assert!(!holds.discovery(now, 1).excluded.contains(&id));
        holds.observe_hold(&id, old, started, started + ADAPTER_DELAY, now);
        assert!(holds.next_probe().is_none());

        let delays: BTreeSet<_> = (0..16)
            .map(|index| {
                let id_agent = AgentId(format!("fixture-{index}"));
                let delay = resume_delay(&id_agent);
                if !delay.is_zero() {
                    assert!(matches!(
                        holds.admit(
                            &id,
                            &id_agent,
                            Some(current.clone()),
                            None,
                            now + delay - Duration::from_millis(1),
                            true
                        ),
                        Admission::Hold
                    ));
                }
                assert!(matches!(
                    holds.admit(
                        &id,
                        &id_agent,
                        Some(current.clone()),
                        None,
                        now + delay,
                        true
                    ),
                    Admission::Proceed
                ));
                delay
            })
            .collect();
        assert!(delays.len() > 1);
        assert!(*delays.last().unwrap() < Duration::from_secs(30));
    }
}

#[tokio::test]
async fn ambiguous_or_mismatched_proof_cannot_seed_a_hold() {
    use crate::sandbox::agent_worker_tests::context_overlap_fixture::parents;
    let (_, mut sandbox, _, _) = parents().await;
    let runtime = sandbox.runtime.as_ref().unwrap().clone();
    let holds = StartupHolds::default();
    let now = Instant::now();
    let startup_error = |generation| {
        Err::<(), _>(AdapterError::RuntimeStartupHold(
            RuntimeStartupFailure::new(RuntimeStartupFailureClass::ExitedBeforeServing, 3)
                .with_probe(RuntimeStartupProbe {
                    runtime: RuntimeHandle::new(&runtime.runtime_id),
                    sandbox_id: sandbox.id.to_string(),
                    generation,
                    next_probe_at: now + ADAPTER_DELAY,
                }),
        ))
    };
    // Legacy runtime data lacks an exact process identity.
    holds.observe(
        &sandbox,
        now,
        &startup_error(runtime.provision_generation.unwrap()),
        now,
    );
    assert!(!holds.pending());
    sandbox.runtime.as_mut().unwrap().boot_id = Some("fixture-process".into());
    holds.observe(
        &sandbox,
        now,
        &startup_error(runtime.provision_generation.unwrap() + 1),
        now,
    );
    assert!(!holds.pending());
    holds.observe(
        &sandbox,
        now,
        &Err::<(), _>(AdapterError::Unavailable("fixture".into())),
        now,
    );
    assert!(!holds.pending());
}

#[tokio::test(start_paused = true)]
async fn hold_capacity_and_expiry_bound_discovery_without_evicting_live_entries() {
    let holds = StartupHolds::default();
    let now = Instant::now();
    for index in 0..=MAX_HOLDS {
        holds.observe_hold(
            &SandboxId(format!("fixture-{index}")),
            binding("initial"),
            now,
            now + ADAPTER_DELAY,
            now,
        );
    }
    let snapshot = holds.discovery(now, 1);
    assert_eq!(snapshot.excluded.len(), MAX_HOLDS);
    assert!(!snapshot
        .excluded
        .contains(&SandboxId(format!("fixture-{MAX_HOLDS}"))));
    assert_eq!(
        holds.refresh_candidates(now + REFRESH_INTERVAL).len(),
        MAX_REFRESH_PER_SWEEP
    );
    assert!(holds
        .refresh_candidates(now + MAX_OBSERVATION_AGE)
        .is_empty());
    assert!(!holds.pending());
}

//! Real admission and control transactions at a fixed retained-home boundary.
use super::*;
use futures_util::FutureExt;
use layer2_core::admission::AdmissionCeilings;
use std::{
    future::Future,
    panic::AssertUnwindSafe,
    time::{Duration, Instant},
};

const HOME_LIMIT: u16 = 2;
const SAMPLES: usize = 100;

#[derive(Clone, Copy, Debug)]
enum Control {
    Stop,
    Cancel,
    Approval,
}

fn admission(index: usize, owner: &str) -> SandboxAdmission {
    let mut sandbox = crate::store::sandbox_admission_mongo_integration_tests::sandbox(
        &format!("sbx_capacity_control_{index}"),
        "unused",
    );
    sandbox.owner = SubjectId::new(owner);
    sandbox.name = "capacity-control".into();
    sandbox.origin = SandboxOrigin::Lifecycle {
        operation_id: format!("capacity-control-{index}"),
    };
    SandboxAdmission::new(
        sandbox,
        SandboxAdmissionName::Allocate("capacity-control".into()),
        2,
        NOW.into(),
    )
    .unwrap()
}

struct Saturated {
    store: MongoSandboxStore,
    peer: MongoSandboxStore,
    accepted: SandboxAdmission,
    pending: Sandbox,
}

impl Saturated {
    async fn fill(store: MongoSandboxStore, name: &str) -> Self {
        let store = store.with_global_sandbox_limit(HOME_LIMIT);
        super::super::ceilings_tests::configure(
            &store,
            AdmissionCeilings {
                retained_homes: Some(u32::from(HOME_LIMIT)),
                active_sessions: Some(1),
                owner_active_sessions: Some(1),
                ..Default::default()
            },
        )
        .await;
        let peer_client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let peer = MongoSandboxStore::open(&peer_client, name)
            .await
            .unwrap()
            .with_global_sandbox_limit(HOME_LIMIT);
        assert_eq!(store.sandboxes.count_documents(doc! {}).await.unwrap(), 1);
        let first = admission(0, "capacity-owner-a");
        let second = admission(1, "capacity-owner-b");
        let (a, b) = tokio::join!(
            store.admit_sandbox(first.clone()),
            peer.admit_sandbox(second.clone())
        );
        let (accepted, pending) = match (a, b) {
            (
                Ok(SandboxAdmissionResult::Created(row)),
                Err(SandboxStoreError::GlobalCapacityReached),
            ) => (first, row),
            (
                Err(SandboxStoreError::GlobalCapacityReached),
                Ok(SandboxAdmissionResult::Created(row)),
            ) => (second, row),
            other => panic!("one remaining home permits exactly one admission: {other:?}"),
        };
        assert_eq!(pending.status, SandboxStatus::Provisioning);
        assert!(pending.runtime.is_none() && pending.credential.current.is_none());
        assert_eq!(
            store.get_sandbox(&pending.id).await.unwrap(),
            Some(pending.clone())
        );
        assert_eq!(
            store
                .sandbox_events(&pending.id, 0, 20)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            peer.admit_sandbox(accepted.clone()).await.unwrap(),
            SandboxAdmissionResult::Existing(pending.clone())
        );
        Self {
            store,
            peer,
            accepted,
            pending,
        }
    }

    async fn control<T, E: std::fmt::Debug>(
        &self,
        operation: impl Future<Output = Result<T, E>>,
    ) -> (T, Duration) {
        // Additional owners keep attempting creation while an already admitted
        // Agent uses a legal control. Both paths share the real Mongo fence.
        let refuse = async {
            for index in 2..=5 {
                assert_eq!(
                    self.peer
                        .admit_sandbox(admission(index, &format!("overflow-owner-{index}")))
                        .await
                        .unwrap_err(),
                    SandboxStoreError::GlobalCapacityReached
                );
            }
        };
        let measured = async {
            let start = Instant::now();
            let result = operation.await.unwrap();
            (result, start.elapsed())
        };
        let (outcome, ()) = tokio::join!(measured, refuse);
        outcome
    }

    async fn assert_reservations(&self) {
        assert_eq!(
            self.store.sandboxes.count_documents(doc! {}).await.unwrap(),
            HOME_LIMIT as u64
        );
        assert_eq!(self.store.agents.count_documents(doc! {}).await.unwrap(), 1);
        assert_eq!(
            self.peer.get_sandbox(&self.pending.id).await.unwrap(),
            Some(self.pending.clone())
        );
        assert_eq!(
            self.peer
                .admit_sandbox(self.accepted.clone())
                .await
                .unwrap(),
            SandboxAdmissionResult::Existing(self.pending.clone())
        );
        assert_eq!(
            self.store
                .sandbox_events(&self.pending.id, 0, 20)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(self.pending.runtime.is_none());
    }
}

async fn sample(control: Control, name: &str, client: &Client) -> (Duration, u32, u32) {
    let (capacity, elapsed) = match control {
        Control::Stop => {
            let (store, agent, _, _) = composed(name, client).await;
            let capacity = Saturated::fill(store, name).await;
            let request = AgentStopRequest {
                target: AgentControlTarget::from(&agent),
                reason: AgentStopReason::Owner,
                requested_at: layer2_core::now_rfc3339(),
            };
            let (scheduled, elapsed) = capacity
                .control(capacity.store.schedule_agent_stop(request.clone()))
                .await;
            let AgentControlResult::Scheduled(after) = scheduled else {
                panic!("first stop must schedule")
            };
            assert_eq!(after.requested_stop_reason, Some(AgentStopReason::Owner));
            let logs = capacity.store.agent_log(&agent.id, 0, 200).await.unwrap();
            assert_eq!(
                capacity.peer.schedule_agent_stop(request).await.unwrap(),
                AgentControlResult::Existing(after.clone())
            );
            assert_eq!(
                capacity.store.get_agent(&agent.id).await.unwrap(),
                Some(after)
            );
            assert_eq!(
                capacity.peer.agent_log(&agent.id, 0, 200).await.unwrap(),
                logs
            );
            (capacity, elapsed)
        }
        Control::Cancel => {
            let (store, _, _, request, source) =
                super::super::queued_cancellation_tests::queued(name, client).await;
            let capacity = Saturated::fill(store, name).await;
            let (result, elapsed) = capacity
                .control(capacity.store.cancel_queued_response(request.clone()))
                .await;
            let QueuedCancellationOutcome::Applied(cancelled) = result else {
                panic!("queued cancellation must apply")
            };
            assert_eq!(cancelled.response.status, AgentResponseStatus::Cancelled);
            assert_eq!(cancelled.response.receipt, source.receipt);
            assert!(
                matches!(capacity.peer.cancel_queued_response(request).await.unwrap(), QueuedCancellationOutcome::Applied(replay) if replay == cancelled)
            );
            (capacity, elapsed)
        }
        Control::Approval => {
            let (store, agent, mut request, _) = composed(name, client).await;
            operator_stop_controls::prepare_approval(&store, &agent, &mut request).await;
            let capacity = Saturated::fill(store, name).await;
            let (accepted, elapsed) = capacity
                .control(capacity.store.reserve_targeted_control(request.clone()))
                .await;
            assert_eq!(
                capacity
                    .peer
                    .reserve_targeted_control(request)
                    .await
                    .unwrap(),
                accepted
            );
            assert_eq!(
                capacity
                    .store
                    .agent_commands
                    .count_documents(doc! {"explicit_control":{"$exists":true}})
                    .await
                    .unwrap(),
                1
            );
            (capacity, elapsed)
        }
    };
    capacity.assert_reservations().await;
    (
        elapsed,
        capacity.pending.profile.content.cpu_millis,
        capacity.pending.profile.content.memory_mib,
    )
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn tc070_admitted_controls_remain_bounded_at_retained_home_capacity() {
    for control in [Control::Stop, Control::Cancel, Control::Approval] {
        let mut samples = Vec::new();
        let mut requested_cpu_millis = 0;
        let mut requested_memory_mib = 0;
        for _ in 0..SAMPLES {
            let (client, name) = database("tc070_control").await;
            let result = AssertUnwindSafe(sample(control, &name, &client))
                .catch_unwind()
                .await;
            let cleanup = client.database(&name).drop().await;
            let (elapsed, cpu, memory) = match result {
                Ok(value) => {
                    cleanup.unwrap();
                    value
                }
                Err(original) => {
                    if let Err(error) = cleanup {
                        eprintln!("fixture cleanup after failure: {error}");
                    }
                    std::panic::resume_unwind(original);
                }
            };
            samples.push(elapsed);
            requested_cpu_millis = cpu;
            requested_memory_mib = memory;
        }
        assert_eq!(samples.len(), SAMPLES);
        samples.sort_unstable();
        let p99 = samples[98];
        eprintln!(
            "TC070_CONTROL_BOUNDARY {}",
            serde_json::json!({
                "control":format!("{control:?}"),"fixture_retained_home_limit":HOME_LIMIT,
                "successful":samples.len(),"typed_overflow_refusals":samples.len()*5,
                "overshoot":0,"pending_cpu_millis_per_fixture":requested_cpu_millis,
                "pending_memory_mib_per_fixture":requested_memory_mib,
                "admission_p99_us":p99.as_micros(),"admission_max_us":samples[99].as_micros(),
            })
        );
        assert!(p99 <= Duration::from_secs(1), "{control:?} p99 {p99:?}");
    }
}

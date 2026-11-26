//! A transient member heartbeat failure skips one tick only while the
//! established membership deadline holds; it never extends any deadline.
use super::*;
use crate::agent_ownership::HEARTBEAT_INTERVAL_MS;
use std::sync::Mutex as StdMutex;
use tracing::instrument::WithSubscriber;

const TICK: Duration = Duration::from_millis(HEARTBEAT_INTERVAL_MS);
const FATAL_DECISION: &str = "Unavailable member heartbeat cannot skip maintenance";

#[derive(Clone, Default)]
struct Output(Arc<StdMutex<Vec<u8>>>);

impl std::io::Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;

    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

impl Output {
    fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
        let output = Self::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_writer(output.clone())
            .finish();
        (output, tracing::subscriber::set_default(subscriber))
    }

    fn decisions(&self) -> Vec<serde_json::Value> {
        let captured = String::from_utf8(self.0.lock().unwrap().clone()).unwrap();
        for private in [
            "gated heartbeat",
            "heartbeat not committed",
            "heartbeat unavailable",
            "synthetic invalid member",
            "renewal-member",
            "agent-000",
            "member.example.test",
        ] {
            assert!(!captured.contains(private), "{captured}");
        }
        captured
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|event| event["fields"]["message"] == FATAL_DECISION)
            .collect()
    }

    fn assert_decision(
        &self,
        reason: &str,
        prior_skipped_ticks: u64,
        flags: (bool, bool),
        remaining_ms: u64,
        deadline_elapsed: bool,
    ) {
        let decisions = self.decisions();
        assert_eq!(decisions.len(), 1, "{decisions:?}");
        let event = &decisions[0];
        assert_eq!(event["level"], "ERROR");
        let fields = &event["fields"];
        assert_eq!(fields["stage"], "member_heartbeat");
        assert_eq!(fields["scope"], "process");
        assert_eq!(fields["outcome"], "fatal");
        assert_eq!(fields["reason"], reason);
        assert_eq!(
            fields["prior_skipped_ticks"].as_u64(),
            Some(prior_skipped_ticks)
        );
        assert_eq!(fields["draining"].as_bool(), Some(flags.0));
        assert_eq!(fields["failed"].as_bool(), Some(flags.1));
        assert_eq!(fields["member_remaining_ms"].as_u64(), Some(remaining_ms));
        assert_eq!(
            fields["member_deadline_elapsed"].as_bool(),
            Some(deadline_elapsed)
        );
        assert_eq!(fields.as_object().unwrap().len(), 10, "{fields}");
    }
}

async fn renewing_fixture() -> Fixture {
    let fixture = Fixture::new(2).await;
    *fixture.store.reply.lock().unwrap() = Reply::Renew;
    fixture.store.all.store(true, Ordering::SeqCst);
    fixture
}

fn assert_serving(fixture: &Fixture) {
    assert!(fixture.manager.is_ready());
    assert!(!*fixture.manager.failure_signal().borrow());
    for authority in &fixture.owners {
        assert!(authority.is_current());
        assert!(!authority.receiver_cancelled());
    }
}

#[tokio::test(start_paused = true)]
async fn transient_heartbeat_failure_skips_one_tick_and_next_heartbeat_restores_renewal() {
    // Late committed reply, unavailable committed reply, uncommitted failure.
    for mode in [1, 6, 7] {
        let fixture = renewing_fixture().await;
        let (output, _guard) = Output::capture();
        let original = fixture.leases().await;
        tokio::time::advance(TICK).await;
        fixture.store.heartbeat_mode.store(mode, Ordering::SeqCst);
        assert!(fixture.manager.maintain().await.unwrap().is_empty());
        assert_serving(&fixture);
        assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.leases().await, original);

        fixture.store.heartbeat_mode.store(0, Ordering::SeqCst);
        fixture.manager.maintain().await.unwrap();
        assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 2);
        for (before, after) in original.iter().zip(fixture.leases().await) {
            assert_eq!(after.token, before.token);
            assert!(after.expires_at_ms > before.expires_at_ms);
        }
        // Past the original lease, the renewed membership and owners still hold.
        tokio::time::advance(Duration::from_secs(13)).await;
        assert_serving(&fixture);
        assert!(output.decisions().is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn skipped_ticks_extend_neither_membership_nor_any_authority_then_fail_as_before() {
    for mode in [6, 7] {
        let fixture = renewing_fixture().await;
        let original = fixture.leases().await;
        let started = Instant::now();
        fixture.store.heartbeat_mode.store(mode, Ordering::SeqCst);
        for _ in 0..4 {
            tokio::time::advance(TICK).await;
            assert!(fixture.manager.maintain().await.unwrap().is_empty());
            assert_serving(&fixture);
        }
        assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 0);
        assert_eq!(fixture.leases().await, original);
        // Registration and acquisition established 15 s deadlines at start.
        tokio::time::advance(
            Duration::from_secs(15) - started.elapsed() - Duration::from_millis(1),
        )
        .await;
        assert_serving(&fixture);
        tokio::time::advance(Duration::from_millis(1)).await;
        assert!(!fixture.manager.is_ready());
        assert!(fixture.owners.iter().all(|owner| !owner.is_current()));
        assert!(matches!(
            fixture.manager.maintain().await,
            Err(OwnershipError::Stale)
        ));
        fixture.assert_failed();
        assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 0);
        assert!(fixture
            .owners
            .iter()
            .all(|owner| !owner.receiver_cancelled()));
    }
}

#[tokio::test(start_paused = true)]
async fn skipped_heartbeat_declines_new_acquisition_until_a_heartbeat_succeeds() {
    for mode in [1, 6, 7] {
        let fixture = renewing_fixture().await;
        let new = AgentId("agent-new".into());
        tokio::time::advance(TICK).await;
        fixture.store.heartbeat_mode.store(mode, Ordering::SeqCst);
        assert!(fixture.manager.maintain().await.unwrap().is_empty());
        // Existing authority is still returned; nothing new is adopted.
        let existing = fixture.owners[0].clone();
        let held = fixture.manager.acquire(&existing.token.agent_id).await;
        assert!(Arc::ptr_eq(&held.unwrap().unwrap(), &existing));
        assert!(fixture.manager.acquire(&new).await.unwrap().is_none());
        assert_eq!(fixture.store.owner(&new).await.unwrap(), None);
        assert!(fixture.manager.local(&new).await.is_none());
        assert_serving(&fixture);

        fixture.store.heartbeat_mode.store(0, Ordering::SeqCst);
        tokio::time::advance(TICK).await;
        fixture.manager.maintain().await.unwrap();
        let adopted = fixture.manager.acquire(&new).await.unwrap().unwrap();
        assert!(adopted.is_current());
        let lease = fixture.store.owner(&new).await.unwrap().unwrap();
        assert_eq!(lease.token, adopted.token);
    }
}

#[tokio::test(start_paused = true)]
async fn heartbeat_failure_at_or_after_the_established_deadline_is_fatal() {
    // The bounded call ends 2 s after it starts; membership ends at 15 s.
    for start_ms in [12_999, 13_000, 13_001] {
        let fixture = renewing_fixture().await;
        let (output, _guard) = Output::capture();
        tokio::time::advance(Duration::from_millis(start_ms)).await;
        fixture.store.heartbeat_mode.store(1, Ordering::SeqCst);
        let started = Instant::now();
        let result = fixture.manager.maintain().await;
        match start_ms {
            12_999 => {
                assert!(result.unwrap().is_empty());
                assert_serving(&fixture);
                assert!(output.decisions().is_empty());
                continue;
            }
            13_000 => {
                assert!(matches!(result, Err(OwnershipError::Unavailable(_))));
                output.assert_decision("heartbeat_call_timeout", 0, (false, false), 0, true);
            }
            _ => {
                assert!(matches!(result, Err(OwnershipError::Stale)));
                assert_eq!(started.elapsed(), Duration::from_millis(1_999));
                assert!(output.decisions().is_empty());
            }
        }
        fixture.assert_failed();
        assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn drain_or_process_failure_during_an_unavailable_heartbeat_is_not_skipped() {
    for (drain, fail) in [(false, true), (true, false), (true, true)] {
        for prior_skips in [0, 2] {
            let fixture = renewing_fixture().await;
            let original = fixture.leases().await;
            let deadline = *fixture.manager.member_deadline.lock().unwrap();
            let (output, _guard) = Output::capture();
            fixture.store.heartbeat_mode.store(7, Ordering::SeqCst);
            for _ in 0..prior_skips {
                assert!(fixture.manager.maintain().await.unwrap().is_empty());
            }
            assert!(output.decisions().is_empty());
            fixture.store.heartbeat_mode.store(11, Ordering::SeqCst);
            let running = tokio::spawn({
                let manager = fixture.manager.clone();
                async move { manager.maintain().await }.with_current_subscriber()
            });
            fixture.store.entered.notified().await;
            if drain {
                fixture.manager.stop_admission();
            }
            if fail {
                fixture.manager.fail().await;
            }
            fixture.store.resume.notify_one();
            assert!(matches!(
                running.await.unwrap(),
                Err(OwnershipError::Unavailable(_))
            ));
            fixture.assert_failed();
            assert_eq!(fixture.manager.is_draining(), drain);
            assert_eq!(fixture.store.calls.load(Ordering::SeqCst), 0);
            assert_eq!(fixture.leases().await, original);
            assert_eq!(*fixture.manager.member_deadline.lock().unwrap(), deadline);
            assert_eq!(
                fixture.manager.skipped_heartbeats.load(Ordering::Acquire),
                prior_skips
            );
            output.assert_decision(
                "heartbeat_store_unavailable",
                u64::from(prior_skips),
                (drain, fail),
                15_000,
                false,
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn fatal_observation_distinguishes_submillisecond_membership_from_expiry() {
    for elapsed in [false, true] {
        let fixture = renewing_fixture().await;
        let deadline = *fixture.manager.member_deadline.lock().unwrap();
        let original = fixture.leases().await;
        tokio::time::advance(Duration::from_micros(14_999_500)).await;
        if elapsed {
            tokio::time::advance(Duration::from_micros(500)).await;
        }
        assert_eq!(Instant::now() >= deadline, elapsed);
        fixture.manager.stop_admission();
        let (output, _guard) = Output::capture();
        // Exercise the observation directly; maintain's outer deadline has its own refusal.
        assert!(matches!(
            fixture
                .manager
                .heartbeat_membership(Ok(Err(OwnershipError::Unavailable(
                    "gated heartbeat".into()
                )))),
            Err(OwnershipError::Unavailable(_))
        ));
        output.assert_decision("heartbeat_store_unavailable", 0, (true, false), 0, elapsed);
        assert_eq!(*fixture.manager.member_deadline.lock().unwrap(), deadline);
        for lease in original {
            let current = fixture.store.owner(&lease.token.agent_id).await.unwrap();
            assert_eq!(current, (!elapsed).then_some(lease));
        }
        assert!(!*fixture.manager.failure_signal().borrow());
        assert_eq!(
            fixture.manager.skipped_heartbeats.load(Ordering::Acquire),
            0
        );
    }
}

#[tokio::test(start_paused = true)]
async fn non_transient_heartbeat_failures_do_not_report_a_transient_fatal_decision() {
    for mode in [8, 9, 10] {
        let fixture = renewing_fixture().await;
        fixture.store.heartbeat_mode.store(mode, Ordering::SeqCst);
        let (output, _guard) = Output::capture();
        let error = fixture.manager.maintain().await.err().unwrap();
        assert!(match mode {
            8 => matches!(error, OwnershipError::Stale),
            9 => matches!(error, OwnershipError::IncompatibleProtocol),
            _ => matches!(error, OwnershipError::Invalid(_)),
        });
        fixture.assert_failed();
        assert!(output.decisions().is_empty());
    }
}

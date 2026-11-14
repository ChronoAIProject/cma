//! Independent reads overlap, but neither read alone may authorize effects.
use super::*;
use layer2_core::sandbox::DesktopConnection;
use tokio::task::JoinHandle;

mod gates;
use gates::{Access, Gate};

struct Attempt {
    fixture: Services,
    service: DesktopService,
    credential: Arc<Gate>,
    lineage: Arc<Gate>,
    task: JoinHandle<Result<DesktopConnection, DesktopError>>,
}

impl Attempt {
    async fn new(stage: usize) -> Self {
        let fixture = Services::new(true).await;
        let credential = Gate::new(stage);
        let lineage = Gate::new(stage);
        let service = fixture::service_with_access(
            fixture.stores.left.clone(),
            fixture.receiver.clone(),
            true,
            Arc::new(Access {
                store: fixture.stores.left.clone(),
                gate: lineage.clone(),
            }),
        );
        let (opening, caller, id, proof) = (
            service.clone(),
            fixture.caller.clone(),
            fixture.agents[0].clone(),
            credential.clone(),
        );
        let task = tokio::spawn(async move { opening.open_authorized(&caller, &id, proof).await });
        Self {
            fixture,
            service,
            credential,
            lineage,
            task,
        }
    }

    async fn both_waiting(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            self.credential.entered.notified().await;
            self.lineage.entered.notified().await;
        })
        .await
        .expect("both independent current reads must start before either is released");
        assert!(!self.task.is_finished());
        assert_eq!(self.fixture.receiver.frames(), 0);
    }

    async fn occupied(&self) -> bool {
        self.fixture
            .stores
            .right
            .has_open_sandbox_session(
                &self.fixture.stores.request.sandbox_id,
                &layer2_core::now_rfc3339(),
            )
            .await
            .unwrap()
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn current_reads_overlap_but_both_gate_acquisition_and_publication() {
    for stage in [1, 2] {
        for credential_first in [true, false] {
            let mut attempt = Attempt::new(stage).await;
            attempt.both_waiting().await;
            assert_eq!(attempt.occupied().await, stage == 2);
            let (first, last) = if credential_first {
                (&attempt.credential, &attempt.lineage)
            } else {
                (&attempt.lineage, &attempt.credential)
            };
            first.release();
            // One completion must not expose a socket while the other read is held.
            assert!(
                tokio::time::timeout(Duration::from_millis(20), &mut attempt.task)
                    .await
                    .is_err()
            );
            assert_eq!(attempt.fixture.receiver.frames(), 0);
            last.release();
            let mut connection = (&mut attempt.task).await.unwrap().unwrap();
            connection
                .send(DesktopFrame::Binary(vec![1]))
                .await
                .unwrap();
            assert_eq!(attempt.fixture.receiver.frames(), 1);
            attempt.service.close_connection(&mut connection).await;
            assert!(!attempt.occupied().await);
            attempt.fixture.stores.db.drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn either_current_read_refusal_cancels_its_peer_without_publication() {
    for stage in [1, 2] {
        for deny_credential in [true, false] {
            let mut attempt = Attempt::new(stage).await;
            attempt.both_waiting().await;
            let (denied, cancelled) = if deny_credential {
                (&attempt.credential, &attempt.lineage)
            } else {
                (&attempt.lineage, &attempt.credential)
            };
            denied.deny.store(true, Ordering::SeqCst);
            denied.release();
            assert!(
                tokio::time::timeout(Duration::from_secs(2), &mut attempt.task)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err()
            );
            assert_eq!(cancelled.cancelled.load(Ordering::SeqCst), 1);
            assert_eq!(attempt.fixture.receiver.frames(), 0);
            assert!(!attempt.occupied().await);
            attempt.fixture.stores.db.drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn cancellation_drops_both_reads_and_retains_an_installed_claim() {
    for stage in [1, 2] {
        let mut attempt = Attempt::new(stage).await;
        attempt.both_waiting().await;
        attempt.task.abort();
        assert!((&mut attempt.task).await.err().unwrap().is_cancelled());
        assert_eq!(attempt.credential.cancelled.load(Ordering::SeqCst), 1);
        assert_eq!(attempt.lineage.cancelled.load(Ordering::SeqCst), 1);
        assert_eq!(attempt.occupied().await, stage == 2);
        assert_eq!(attempt.fixture.receiver.frames(), 0);
        attempt.fixture.stores.db.drop().await.unwrap();
    }
}

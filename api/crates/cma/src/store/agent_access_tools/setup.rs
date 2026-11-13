//! Read-only MCP discovery shares a fresh snapshot without a commit round trip.
use super::*;

impl MongoAgentAccessToolSource {
    pub(super) async fn setup_snapshot(&self, capability: &str) -> layer3_adapter::Result<()> {
        let jitter = uuid::Uuid::new_v4().as_u128() as u64;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                let delay =
                    (100_u64 << (attempt - 1).min(3)) + (jitter.rotate_left(attempt as u32) & 31);
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            let mut session = self
                .store
                .client
                .start_session()
                .snapshot(true)
                .await
                .map_err(|error| {
                    diagnostic("start_session", driver_category(&error), attempt);
                    unavailable(())
                })?;
            // All reads, including oversized-record fallback, use this session's
            // atClusterTime. No result grants tool-call or mutation authority.
            match self.setup_in(capability, &mut session, true).await {
                Ok(()) => return Ok(()),
                Err(Error::Driver(error)) => {
                    diagnostic("snapshot_read", driver_category(&error), attempt);
                    if retryable_snapshot(&error) {
                        continue;
                    }
                    return Err(unavailable(()));
                }
                Err(Error::Refused) => {
                    diagnostic("snapshot_read", "authority_rejected", attempt);
                    return Err(refused());
                }
                Err(Error::Unavailable) => {
                    diagnostic("snapshot_read", "storage_unavailable", attempt);
                    return Err(unavailable(()));
                }
            }
        }
        diagnostic("retry", "retry_exhausted", TRANSACTION_ATTEMPTS - 1);
        Err(unavailable(()))
    }
}

fn retryable_snapshot(error: &mongodb::error::Error) -> bool {
    // These read-only attempts have no unknown write outcome. A fresh snapshot
    // can recover SnapshotUnavailable / SnapshotTooOld within the existing budget.
    is_transient(error)
        || matches!(error.kind.as_ref(),
        mongodb::error::ErrorKind::Command(command) if matches!(command.code, 246 | 286))
}

fn diagnostic(stage: &'static str, category: &'static str, attempt: usize) {
    let span = tracing::warn_span!(parent: None, "agent_access_setup");
    if span.is_disabled() {
        return;
    }
    let _entered = span.enter();
    tracing::warn!(
        operation = "setup",
        stage,
        category,
        attempt = attempt + 1,
        "Agent access tool authority snapshot did not complete"
    );
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};

    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Log {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn setup_diagnostics_exclude_private_parent_when_root_is_enabled_or_disabled() {
        for enable_root in [true, false] {
            let log = Log::default();
            let writer = log.clone();
            let subscriber = tracing_subscriber::registry().with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_writer(move || writer.clone())
                    .with_filter(tracing_subscriber::filter::filter_fn(move |metadata| {
                        enable_root || metadata.name() != "agent_access_setup"
                    })),
            );
            let _default = subscriber.set_default();
            let private = tracing::warn_span!("private_caller", canary = "private_canary");
            let _entered = private.enter();
            super::diagnostic("snapshot_read", "authority_rejected", 0);
            let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
            assert!(!text.contains("private_canary"));
            assert_eq!(text.contains("authority_rejected"), enable_root);
            // Prove that suppressing a disabled root did not disable events.
            tracing::warn!("event_filter_control");
            let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
            assert!(text.contains("event_filter_control"));
        }
    }
}

use super::*;
use std::future::Future;

#[derive(Clone)]
pub(crate) struct Session {
    pub(super) receiver: Arc<DesktopReceiver>,
    pub(super) revision: u64,
    pub(super) owner: Option<DesktopOwnership>,
}

impl Session {
    fn deadline(&self) -> Result<Option<Instant>> {
        let state = self.receiver.state.lock().unwrap();
        if state.poisoned || state.revision != self.revision {
            return Err(refused());
        }
        match &self.owner {
            None if !state.legacy_closed => Ok(None),
            Some(owner) if self.receiver.live(&state, owner) => Ok(state.deadline),
            _ => Err(refused()),
        }
    }

    /// Every frame read and send, including a blocked sink, goes through here.
    pub(crate) async fn run<T>(&self, operation: impl Future<Output = T>) -> Result<T> {
        let mut changed = self.receiver.changed.subscribe();
        tokio::pin!(operation);
        loop {
            let deadline = self.deadline()?;
            tokio::select! {
                biased;
                _ = async {
                    match deadline {
                        Some(until) => tokio::time::sleep_until(until).await,
                        None => std::future::pending().await,
                    }
                } => return Err(refused()),
                _ = changed.changed() => {},
                result = &mut operation => {
                    self.deadline()?;
                    return Ok(result);
                },
            }
        }
    }
}

pub(super) struct Finish(pub Session);

impl Drop for Finish {
    fn drop(&mut self) {
        if self.0.owner.is_none() {
            return;
        }
        let receiver = &self.0.receiver;
        let mut state = receiver.state.lock().unwrap();
        if state.revision == self.0.revision {
            // No serial gate here: takeover/revoke may hold it while joining
            // this exact task. Persist the terminal floor under the state lock.
            let _ = receiver.retire(&mut state);
        }
    }
}

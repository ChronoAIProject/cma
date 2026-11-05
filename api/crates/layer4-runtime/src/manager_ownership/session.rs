use super::*;

#[derive(Clone)]
pub(crate) struct OwnerSession {
    pub(super) gate: Arc<Gate>,
    pub(super) owner: Option<TransportOwner>,
    pub(super) revision: u64,
}

impl OwnerSession {
    pub(crate) fn owner(&self) -> Option<TransportOwner> {
        self.owner.clone()
    }

    pub(crate) async fn permit(&self) -> Result<OwnershipPermit> {
        let effect = self.gate.effects.clone().read_owned().await;
        let state = self.gate.state.lock().await;
        let current = match &self.owner {
            None => !state.record.required && state.record.revision == self.revision,
            Some(owner) => {
                state.record.required
                    && !state.record.retired
                    && state.record.owner.as_ref() == Some(owner)
                    && state
                        .deadline
                        .is_some_and(|deadline| deadline > Instant::now())
            }
        };
        if current {
            Ok(OwnershipPermit {
                _effect: effect,
                deadline: state.deadline,
            })
        } else {
            Err(refused())
        }
    }

    pub(crate) async fn expired(&self) {
        let mut changed = self.gate.changed.subscribe();
        loop {
            changed.borrow_and_update();
            let until = {
                let Ok(permit) = self.permit().await else {
                    return;
                };
                permit.deadline()
            };
            tokio::select! {
                result = changed.changed() => if result.is_err() { return },
                _ = async {
                    match until {
                        Some(until) => tokio::time::sleep_until(until).await,
                        None => std::future::pending().await,
                    }
                } => (),
            }
        }
    }
}

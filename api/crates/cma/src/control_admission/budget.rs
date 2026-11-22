use super::{Class, ControlAdmissionConfig, Refusal};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

type Identity = [u8; 32];

#[derive(Default)]
struct Held {
    callers: BTreeMap<Identity, [bool; 2]>,
    targets: BTreeSet<(Identity, Class)>,
}

/// Admission slots sized for the proposed connection-demand envelope. These
/// bound HTTP concurrency only until stage fan-out/deferred work is qualified.
pub(crate) struct ResourceBudget {
    lifecycle: [Arc<Semaphore>; 2],
    held: Mutex<Held>,
    max_callers: usize,
}

impl ResourceBudget {
    pub(crate) fn new(config: ControlAdmissionConfig) -> Arc<Self> {
        Arc::new(Self {
            lifecycle: [
                Arc::new(Semaphore::new(config.stop_credits)),
                Arc::new(Semaphore::new(config.delete_credits)),
            ],
            held: Mutex::new(Held::default()),
            max_callers: config.max_callers,
        })
    }

    pub(crate) fn try_stage(
        self: &Arc<Self>,
        caller: &str,
        target: &str,
        class: Class,
        demand: u32,
    ) -> Result<StagePermit, Refusal> {
        if demand != 1 {
            return Err(Refusal::Capacity);
        }
        let caller = identity(caller);
        let target = identity(target);
        let mut held = self.held.lock().unwrap();
        if held
            .callers
            .get(&caller)
            .is_some_and(|classes| classes[class.index()])
            || held.targets.contains(&(target, class))
        {
            return Err(Refusal::Caller(1));
        }
        if !held.callers.contains_key(&caller) && held.callers.len() >= self.max_callers {
            return Err(Refusal::Capacity);
        }
        let permit = self.lifecycle[class.index()]
            .clone()
            .try_acquire_many_owned(demand)
            .map_err(|_| Refusal::Capacity)?;
        held.callers.entry(caller).or_default()[class.index()] = true;
        held.targets.insert((target, class));
        Ok(StagePermit {
            budget: self.clone(),
            caller,
            target,
            class,
            _permit: permit,
        })
    }
}

/// Keep the permit with its executing future until that future is joined.
/// Dropping only an HTTP observer must not release a still-running stage.
pub(crate) struct StagePermit {
    budget: Arc<ResourceBudget>,
    caller: Identity,
    target: Identity,
    class: Class,
    _permit: OwnedSemaphorePermit,
}

impl Drop for StagePermit {
    fn drop(&mut self) {
        let mut held = self.budget.held.lock().unwrap();
        let classes = held
            .callers
            .get_mut(&self.caller)
            .expect("stage owns its caller entry");
        classes[self.class.index()] = false;
        if classes.iter().all(|value| !value) {
            held.callers.remove(&self.caller);
        }
        held.targets.remove(&(self.target, self.class));
    }
}

pub(super) fn identity(value: &str) -> Identity {
    use sha2::{Digest, Sha256};
    Sha256::digest(value.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_reserve_and_each_caller_share_survive_delete_saturation() {
        let budget = ResourceBudget::new(ControlAdmissionConfig::default());
        let deletes: Vec<_> = (0..4)
            .map(|i| {
                budget
                    .try_stage(
                        &format!("caller-{i}"),
                        &format!("agent-{i}"),
                        Class::Delete,
                        1,
                    )
                    .unwrap()
            })
            .collect();
        assert!(matches!(
            budget.try_stage("extra", "extra", Class::Delete, 1),
            Err(Refusal::Capacity)
        ));
        assert!(matches!(
            budget.try_stage("caller-0", "another", Class::Delete, 1),
            Err(Refusal::Caller(_))
        ));
        let stop = budget
            .try_stage("caller-0", "agent-0", Class::Stop, 1)
            .unwrap();
        let other_stop = budget
            .try_stage("other", "agent-other", Class::Stop, 1)
            .unwrap();
        assert!(matches!(
            budget.try_stage("third", "third", Class::Stop, 1),
            Err(Refusal::Capacity)
        ));
        drop((deletes, stop, other_stop));
        assert!(budget.held.lock().unwrap().callers.is_empty());
        assert!(budget.held.lock().unwrap().targets.is_empty());
        assert_eq!(budget.lifecycle[0].available_permits(), 2);
        assert_eq!(budget.lifecycle[1].available_permits(), 4);
    }

    #[test]
    fn target_single_flight_map_capacity_and_stage_weight_fail_closed() {
        let budget = ResourceBudget::new(ControlAdmissionConfig {
            max_callers: 1,
            ..Default::default()
        });
        let permit = budget.try_stage("caller", "agent", Class::Stop, 1).unwrap();
        assert!(matches!(
            budget.try_stage("other", "agent", Class::Stop, 1),
            Err(Refusal::Caller(_))
        ));
        assert!(matches!(
            budget.try_stage("other", "other", Class::Delete, 1),
            Err(Refusal::Capacity)
        ));
        assert!(budget
            .try_stage("caller", "other", Class::Delete, 2)
            .is_err());
        assert!(budget
            .try_stage("caller", "other", Class::Delete, 0)
            .is_err());
        drop(permit);
        assert!(budget.try_stage("other", "agent", Class::Stop, 1).is_ok());
    }
}

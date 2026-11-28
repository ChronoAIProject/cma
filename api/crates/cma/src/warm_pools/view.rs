use super::{NativePoolStatus, PoolTarget, WarmPoolView};

impl PoolTarget {
    pub fn view(&self, live: Option<&NativePoolStatus>, available: bool) -> WarmPoolView {
        let exact = live.filter(|status| self.binding.as_ref() == Some(&status.binding));
        let observed = exact.is_some_and(|status| {
            status.observed && status.desired_replicas == self.target_replicas
        });
        let ready = exact
            .filter(|_| observed)
            .map(|status| status.ready_replicas);
        let applied = self.applied_revision == self.revision;
        let at_zero = self.target_replicas == 0 && ready == Some(0);
        let status = if !available || self.error.is_some() {
            "unavailable"
        } else if !applied || !observed {
            "pending"
        } else {
            match (self.target_replicas, ready) {
                (0, Some(0)) => "disabled",
                (target, Some(count)) if target > 0 && count >= target => "ready",
                _ => "pending",
            }
        };
        // A later retirement error can hold the pool definition and its homes
        // while the applied zero target and exact provider observation still
        // confirm the spare capacity is drained.
        let retirement_status = self.retiring.then(|| {
            if available && applied && at_zero {
                "drained".into()
            } else {
                "pending".into()
            }
        });
        WarmPoolView {
            id: self.id.clone(),
            revision: self.revision as u64,
            image: self.plan.spec.image.uri.clone(),
            config_sha256: self.plan.config_sha256.clone(),
            cpu_millis: self.plan.spec.cpu_millis,
            memory_mib: self.plan.spec.memory_mib,
            target_replicas: self.target_replicas,
            provider_target_replicas: exact.map(|status| status.desired_replicas),
            ready_replicas: ready,
            observed,
            status: status.into(),
            retirement_status,
            resource_name: self.binding.as_ref().map(|binding| binding.name.clone()),
            error: self.error.clone().or_else(|| {
                if !available {
                    Some("Native warm pool inventory is unavailable.".into())
                } else if self.applied_revision == self.revision
                    && self.target_replicas > 0
                    && ready == Some(0)
                {
                    Some("Native provider reports zero Ready members; the pool is degraded.".into())
                } else {
                    None
                }
            }),
            reconciliation_failure: self.reconciliation_failure.clone(),
            machine: self.machine,
            current_runtime: None,
        }
    }
}

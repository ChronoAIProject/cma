#[path = "worker_batch.rs"]
mod batch;
use std::collections::BTreeMap;
use std::{sync::Arc, time::Instant};

use cmaeg_core::{
    activity::{
        Activity, ActivityConversation, ActivityKind, Actor, ActorKind, ConversationKind,
        SourceKind,
    },
    cloudevent::CloudEvent,
    ids::{ConversationId, EventId, IdempotencyKey},
    seal::{event_ref::activity_digest, EventRef, EventRefPayload, ReplyAddress},
};
use serde_json::json;
use tokio::{sync::watch, task::JoinHandle};

use super::{
    group,
    model::{parse_task_result, TaskResult, TaskResultStatus},
    Error, Gateway, Task,
};

const POLL_INTERVAL_SECONDS: u64 = 1;
const RETENTION_SECONDS: i64 = 7 * 24 * 60 * 60;

pub(crate) struct Worker {
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl Worker {
    pub(crate) fn start(
        state: crate::channels::management::ManagementState,
        ingress: crate::ingress::IngressState,
    ) -> Self {
        let (stop, mut stopped) = watch::channel(false);
        let task = tokio::spawn(async move {
            let gateway = tokio::select! {
                gateway = Gateway::from_records(&state.records, state.master_keys.clone()) => gateway,
                _ = stopped.changed() => None,
            };
            let Some(mut gateway) = gateway else { return };
            gateway.attach_providers(state.providers.clone());
            gateway.attach_ingress(Arc::new(ingress));
            gateway.attach_activity(state.activity.clone());
            let gateway = Arc::new(gateway);
            if state.github.set(gateway.clone()).is_err() || !gateway.config.has_routes() {
                return;
            }
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(POLL_INTERVAL_SECONDS));
            loop {
                tokio::select! {
                    changed = stopped.changed() => {
                        if changed.is_err() || *stopped.borrow() {
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        if let Err(error) = gateway.process_groups().await {
                            tracing::error!(%error, "GitHub execution-group worker scan failed");
                        }
                    }
                }
            }
        });
        Self {
            stop,
            task: Some(task),
        }
    }

    pub(crate) fn stop(&self) {
        let _ = self.stop.send(true);
    }

    pub(crate) async fn drain(&mut self, deadline: Instant) -> bool {
        self.stop();
        let Some(mut task) = self.task.take() else {
            return true;
        };
        match tokio::time::timeout_at(deadline.into(), &mut task).await {
            Ok(Ok(())) => true,
            Ok(Err(_)) => false,
            Err(_) => {
                task.abort();
                let _ = task.await;
                false
            }
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Gateway {
    async fn process_groups(&self) -> Result<(), Error> {
        let groups = self
            .store
            .list_kind::<group::ExecutionGroup>("execution_group")
            .await?;
        let current = super::now();
        for (group_id, owner, _version, group) in groups {
            if let Some(active) = group.active.clone() {
                if let Some(admission_id) = active.admission_id.as_deref() {
                    self.reconcile_group(
                        &group_id,
                        &owner,
                        &group,
                        admission_id,
                        &active.batch_id,
                        &active.metadata,
                    )
                    .await?;
                } else if active.launch_lease_until <= current {
                    let mut claimed = None;
                    if self
                        .mutate_group(&group_id, &owner, |group| {
                            claimed = group.claim_active_launch(current);
                            Ok(claimed.is_some())
                        })
                        .await?
                    {
                        if let Some(batch) = claimed {
                            self.launch_batch(&group_id, &owner, &group, &batch).await;
                        }
                    }
                }
            } else if group.due(current) {
                let mut started = None;
                if self
                    .mutate_group(&group_id, &owner, |group| {
                        started = group.begin_batch(current);
                        Ok(started.is_some())
                    })
                    .await?
                {
                    if let Some(batch) = started {
                        self.launch_batch(&group_id, &owner, &group, &batch).await;
                    }
                }
            }
        }
        Ok(())
    }

    async fn launch_batch(
        &self,
        group_id: &str,
        owner: &str,
        group: &group::ExecutionGroup,
        batch: &group::Batch,
    ) {
        let result = self
            .build_batch_request(owner, group_id, group, batch)
            .await;
        let request = match result {
            Ok(request) => request,
            Err(_) => {
                let _ = self
                    .finish_group(group_id, owner, &batch.batch_id, "owner_action", None, None)
                    .await;
                return;
            }
        };
        let metadata = crate::turns::metadata::render(&request);
        let Some(ingress) = self.ingress.as_ref() else {
            let _ = self
                .finish_group(group_id, owner, &batch.batch_id, "failed", None, None)
                .await;
            return;
        };
        match ingress.launch_github(request, Vec::new()).await {
            crate::ingress::launch::LaunchOutcome::Admitted { admission_id, .. } => {
                let _ = self
                    .attach_admission(group_id, owner, &batch.batch_id, admission_id, metadata)
                    .await;
            }
            crate::ingress::launch::LaunchOutcome::Accepted { response_id } => {
                let _ = self
                    .attach_admission(group_id, owner, &batch.batch_id, response_id, metadata)
                    .await;
            }
            crate::ingress::launch::LaunchOutcome::Refused(_)
            | crate::ingress::launch::LaunchOutcome::Deferred => {
                let _ = self
                    .finish_group(group_id, owner, &batch.batch_id, "owner_action", None, None)
                    .await;
            }
        }
    }

    async fn reconcile_group(
        &self,
        group_id: &str,
        owner: &str,
        group: &group::ExecutionGroup,
        admission_id: &str,
        batch_id: &str,
        metadata: &BTreeMap<String, String>,
    ) -> Result<(), Error> {
        let task = match self.admitted_task(owner, admission_id).await {
            Ok(task) => task,
            Err(Error::NotFound) => return Ok(()),
            Err(error) => return Err(error),
        };
        if matches!(
            task.status.as_str(),
            "succeeded" | "no_change" | "failed" | "owner_action"
        ) {
            let result = if task.status == "succeeded" {
                self.result_for(group, task.response_id.as_deref(), metadata)
                    .await
            } else {
                None
            };
            let status =
                result
                    .as_ref()
                    .map_or(task.status.as_str(), |result| match result.status {
                        TaskResultStatus::Succeeded => "succeeded",
                        TaskResultStatus::NoChange => "no_change",
                    });
            self.finish_group(
                group_id,
                owner,
                batch_id,
                status,
                task.response_id.clone(),
                result,
            )
            .await?;
        }
        Ok(())
    }

    async fn result_for(
        &self,
        group: &group::ExecutionGroup,
        response_id: Option<&str>,
        metadata: &BTreeMap<String, String>,
    ) -> Option<TaskResult> {
        let response_id = response_id?;
        let providers = self.providers.as_ref()?;
        let ingress = self.ingress.as_ref()?;
        let channel = ingress.resolver.resolve(&group.channel_id).await.ok()?;
        let consumer = channel.consumer(None)?;
        let credential = consumer.credential?;
        let identity = crate::providers::RecoveryIdentity {
            response_id: response_id.to_owned(),
            native_agent_id: None,
            metadata: metadata.clone(),
        };
        let snapshot = providers
            .response_snapshot(&consumer.provider.slug, &identity, credential)
            .await;
        match snapshot {
            Ok(snapshot) => parse_task_result(&snapshot.response),
            Err(error) => {
                tracing::warn!(%error, "GitHub task result projection unavailable");
                None
            }
        }
    }

    async fn admitted_task(&self, owner: &str, admission_id: &str) -> Result<Task, Error> {
        let Some(ingress) = self.ingress.as_ref() else {
            return Err(Error::Unavailable);
        };
        let Some(routed) = ingress.routed.as_ref() else {
            return Err(Error::Unavailable);
        };
        let mut task = routed.task(owner, admission_id).await?;
        self.project_activity(owner, &mut task).await
    }

    async fn mutate_group<F>(
        &self,
        group_id: &str,
        owner: &str,
        mut mutate: F,
    ) -> Result<bool, Error>
    where
        F: FnMut(&mut group::ExecutionGroup) -> Result<bool, Error>,
    {
        for _ in 0..8 {
            let (stored_owner, version, mut group) = self
                .store
                .read_versioned::<group::ExecutionGroup>(group_id, "execution_group")
                .await?;
            if stored_owner != owner || group.owner != owner {
                return Err(Error::NotFound);
            }
            if !mutate(&mut group)? {
                return Ok(false);
            }
            match self
                .store
                .update(group_id, owner, "execution_group", version, &group)
                .await
            {
                Ok(_) => return Ok(true),
                Err(Error::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(Error::Unavailable)
    }

    async fn attach_admission(
        &self,
        group_id: &str,
        owner: &str,
        batch_id: &str,
        admission_id: String,
        metadata: BTreeMap<String, String>,
    ) -> Result<(), Error> {
        let current = super::now();
        self.mutate_group(group_id, owner, |group| {
            match group.attach_admission(batch_id, admission_id.clone(), metadata.clone(), current)
            {
                Ok(()) => Ok(true),
                Err(group::GroupError::WrongBatch) => Ok(false),
                Err(error) => Err(super::map_group_error(error)),
            }
        })
        .await
        .map(|_| ())
    }

    async fn finish_group(
        &self,
        group_id: &str,
        owner: &str,
        batch_id: &str,
        status: &str,
        response_id: Option<String>,
        result: Option<TaskResult>,
    ) -> Result<(), Error> {
        let current = super::now();
        self.mutate_group(group_id, owner, |group| {
            match group.finish_batch(
                batch_id,
                status,
                response_id.clone(),
                result.clone(),
                current,
            ) {
                Ok(()) => Ok(true),
                Err(group::GroupError::WrongBatch) => Ok(false),
                Err(error) => Err(super::map_group_error(error)),
            }
        })
        .await
        .map(|_| ())
    }

    pub(super) async fn group_task(
        &self,
        owner: &str,
        group: group::ExecutionGroup,
    ) -> Result<Task, Error> {
        let current = super::now();
        let mut task = Task {
            task_id: group.group_id.clone(),
            status: if group.active.is_some() {
                "running".into()
            } else if !group.pending.is_empty() {
                "queued".into()
            } else {
                group.last_status.clone()
            },
            response_id: group.last_response_id.clone(),
            channel_id: group.channel_id.clone(),
            record_id: group.record_id.clone(),
            event_id: group
                .active
                .as_ref()
                .map(|batch| batch.batch_id.clone())
                .or(group.last_batch_id.clone())
                .unwrap_or_else(|| group.group_id.clone()),
            target_count: 1,
            accepted_at: group
                .active
                .as_ref()
                .map_or(group.updated_at, |batch| batch.started_at),
            retained_until: group
                .updated_at
                .saturating_add(RETENTION_SECONDS)
                .max(current),
            result: group.result.clone(),
        };
        if let Some(admission_id) = group
            .active
            .as_ref()
            .and_then(|batch| batch.admission_id.as_deref())
        {
            match self.admitted_task(owner, admission_id).await {
                Ok(mut admitted) => {
                    admitted.task_id = group.group_id;
                    return Ok(admitted);
                }
                Err(Error::NotFound) => task.status = "recovering".into(),
                Err(error) => return Err(error),
            }
        } else if group.active.is_some() {
            task.status = "recovering".into();
        }
        Ok(task)
    }
}

fn binding_allows(binding: &super::Binding, member: &group::Member, current: i64) -> bool {
    binding.status == "active"
        && binding.verified_until > current
        && binding.authority_revision == member.authority_revision
        && binding.repository_ids.contains(&member.repository_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn binding() -> super::super::Binding {
        super::super::Binding {
            binding_id: "binding-1".into(),
            app_ref: "pilot".into(),
            app_id: 1,
            installation_id: 7,
            account_id: 8,
            status: "active".into(),
            authority_revision: 3,
            verified_until: 200,
            verified_by: "operator".into(),
            verification_ref: "proof".into(),
            repository_ids: vec![42],
        }
    }

    fn member() -> group::Member {
        group::Member {
            receipt_id: "receipt-1".into(),
            delivery_id: "delivery-1".into(),
            event: "issues".into(),
            action: Some("opened".into()),
            installation_id: 7,
            authority_revision: 3,
            repository_id: 42,
            payload: json!({"issue":{"id":1,"number":1}}),
        }
    }

    #[test]
    fn launch_requires_current_binding_revision_and_repository_membership() {
        let binding = binding();
        let member = member();
        assert!(binding_allows(&binding, &member, 100));

        let mut suspended = binding.clone();
        suspended.status = "suspended".into();
        assert!(!binding_allows(&suspended, &member, 100));

        let mut stale = member.clone();
        stale.authority_revision = 2;
        assert!(!binding_allows(&binding, &stale, 100));

        let mut removed = binding.clone();
        removed.repository_ids.clear();
        assert!(!binding_allows(&removed, &member, 100));

        assert!(!binding_allows(&binding, &member, 200));
    }
}

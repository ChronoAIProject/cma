use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::resource_access::{
    plan_share_mutation, ResourceAccessSnapshot, ResourceAccessStoreError, ResourceShare,
    ResourceShareStore, ShareChange, ShareMutation, ShareMutationReceipt,
};
use layer2_core::SandboxId;
use mongodb::bson::doc;
use mongodb::ClientSession;

use super::super::sandbox_mongo::{
    commit_transaction, is_confirmed_body_retry, signed, start_transaction, CommitOutcome,
    MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};
use super::{records, snapshot, ReadError};

#[async_trait::async_trait]
impl ResourceShareStore for MongoSandboxStore {
    async fn share_mutation_receipt(
        &self,
        operation_id: &str,
    ) -> Result<Option<ShareMutationReceipt>, ResourceAccessStoreError> {
        self.resource_share_commands
            .find_one(doc! {"_id": operation_id})
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)?
            .as_ref()
            .map(records::read_receipt)
            .transpose()
    }

    async fn mutate_share(
        &self,
        request: ShareMutation,
    ) -> Result<ShareMutationReceipt, ResourceAccessStoreError> {
        request.validate()?;
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?;
            let receipt = match apply(self, &request, &mut session).await {
                Ok(receipt) => receipt,
                Err(ReadError::Domain(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
                Err(ReadError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    if is_confirmed_body_retry(&error) {
                        continue;
                    }
                    return Err(ResourceAccessStoreError::Unavailable);
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(receipt),
                CommitOutcome::Retry(_) => continue,
                // The caller retries the same key to recover its immutable
                // audit/response. Never acknowledge an unproven write.
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(ResourceAccessStoreError::Unavailable)
                }
            }
        }
        Err(ResourceAccessStoreError::Conflict)
    }
    async fn resource_share(
        &self,
        id: &str,
    ) -> Result<Option<ResourceShare>, ResourceAccessStoreError> {
        let document = self
            .resource_shares
            .find_one(doc! {"_id":id})
            .await
            .map_err(|_| ResourceAccessStoreError::Unavailable)?;
        document
            .as_ref()
            .map(|document| {
                let sandbox_id = document
                    .get_str("sandbox_id")
                    .map_err(|_| ResourceAccessStoreError::Corrupt)?;
                records::read_share(&SandboxId(sandbox_id.into()), document)
            })
            .transpose()
    }
}

async fn apply_inner(
    store: &MongoSandboxStore,
    request: &ShareMutation,
    session: &mut ClientSession,
    guarded: bool,
) -> Result<ShareMutationReceipt, ReadError> {
    if let Some(receipt) = store
        .resource_share_commands
        .find_one(doc! {"_id":request.operation_id()})
        .session(&mut *session)
        .await?
    {
        return Ok(request.replay(records::read_receipt(&receipt)?)?);
    }
    if !guarded {
        super::execution::refuse_legacy_change(store, request, session).await?;
    }
    fence_recipients(store, request, session).await?;
    let snapshot = snapshot::read(store, &request.target(), session).await?;
    let sandbox_id = &snapshot.lineage.sandbox.id;
    let id = match &request.change {
        ShareChange::Create { .. } | ShareChange::CreateLink { .. } => &request.proposed_id,
        ShareChange::SetMode { share_id, .. }
        | ShareChange::Revoke { share_id }
        | ShareChange::RedeemLink { share_id, .. } => share_id,
    };
    let previous = store
        .resource_shares
        .find_one(doc! {"_id":id})
        .session(&mut *session)
        .await?
        .as_ref()
        .map(|document| {
            if document
                .get_str("sandbox_id")
                .map_err(|_| ResourceAccessStoreError::Corrupt)?
                != sandbox_id.as_str()
            {
                return Err(ResourceAccessStoreError::NotFound);
            }
            records::read_share(sandbox_id, document)
        })
        .transpose()?;
    let count = store
        .resource_shares
        .count_documents(doc! {"sandbox_id":sandbox_id.as_str(),"revoked":false})
        .session(&mut *session)
        .await?;
    let count = usize::try_from(count).map_err(|_| ResourceAccessStoreError::Capacity)?;
    let plan = plan_share_mutation(request, &snapshot, previous.as_ref(), count)?;
    fence_lineage(store, &snapshot, session).await?;
    let policy = records::policy_document(&plan.policy)?;
    if let Some(old) = snapshot.policy {
        let revision = signed(old.revision, "policy revision")
            .map_err(|_| ResourceAccessStoreError::Corrupt)?;
        let result=store.resource_policies.replace_one(doc!{"_id":sandbox_id.as_str(),"revision":revision,"document_schema_version":DOCUMENT_SCHEMA_VERSION},policy).session(&mut *session).await?;
        if result.matched_count != 1 {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
    } else {
        store
            .resource_policies
            .insert_one(policy)
            .session(&mut *session)
            .await?;
    }
    let share = records::share_document(sandbox_id, &plan.receipt.share)?;
    if let Some(old) = previous {
        let revision = signed(old.revision, "share revision")
            .map_err(|_| ResourceAccessStoreError::Corrupt)?;
        let result=store.resource_shares.replace_one(doc!{"_id":&old.id,"revision":revision,"document_schema_version":DOCUMENT_SCHEMA_VERSION},share).session(&mut *session).await?;
        if result.matched_count != 1 {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
    } else {
        store
            .resource_shares
            .insert_one(share)
            .session(&mut *session)
            .await?;
    }
    store
        .resource_share_commands
        .insert_one(records::receipt_document(&plan.receipt)?)
        .session(&mut *session)
        .await?;
    Ok(plan.receipt)
}

async fn fence_recipients(
    store: &MongoSandboxStore,
    request: &ShareMutation,
    session: &mut ClientSession,
) -> Result<(), ReadError> {
    let subjects: Vec<_> = match &request.change {
        ShareChange::CreateLink {
            audience: layer2_core::resource_access::AgentShareAudience::Users { subjects },
            ..
        } => subjects.iter().collect(),
        ShareChange::RedeemLink { subject, .. } => vec![subject],
        _ => Vec::new(),
    };
    let database = store.client.database(&store.sandboxes.namespace().db);
    for subject in subjects {
        if crate::store::account_deletions::fence_subject_in_session(&database, subject, session)
            .await?
        {
            return Err(ResourceAccessStoreError::ShareRecipientUnavailable.into());
        }
    }
    Ok(())
}

pub(in crate::store) async fn fence_lineage(
    store: &MongoSandboxStore,
    snapshot: &ResourceAccessSnapshot,
    session: &mut ClientSession,
) -> Result<(), ReadError> {
    let lineage = &snapshot.lineage;
    let mut rows = vec![(
        &store.sandboxes,
        lineage.sandbox.id.as_str(),
        lineage.sandbox.revision,
    )];
    if let Some(workspace) = &lineage.workspace {
        rows.push((&store.workspaces, workspace.id.as_str(), workspace.revision));
    }
    if let Some(worktree) = &lineage.worktree {
        rows.push((&store.worktrees, worktree.id.as_str(), worktree.revision));
    }
    if let Some(agent) = &lineage.agent {
        rows.push((&store.agents, agent.id.as_str(), agent.revision));
    }
    for (collection, id, revision) in rows {
        let revision =
            signed(revision, "resource revision").map_err(|_| ResourceAccessStoreError::Corrupt)?;
        let mut filter = doc! {"_id": id, "revision": revision, "document_schema_version": DOCUMENT_SCHEMA_VERSION};
        if collection.name() == store.agents.name() {
            // The snapshot validated the Agent's envelope. Fence its exact
            // revision without changing the version, log or head.
            filter.insert(
                "document_schema_version",
                doc! {"$in": [DOCUMENT_SCHEMA_VERSION, 2_i64]},
            );
        }
        let result = collection
            .update_one(
                crate::store::work_schema::for_collection(collection, filter),
                doc! {"$inc": {"resource_share_guard": 1_i64}},
            )
            .session(&mut *session)
            .await?;
        if result.matched_count != 1 {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
    }
    Ok(())
}

async fn apply(
    store: &MongoSandboxStore,
    request: &ShareMutation,
    session: &mut ClientSession,
) -> Result<ShareMutationReceipt, ReadError> {
    apply_inner(store, request, session, false).await
}
pub(super) async fn apply_execution_finalization(
    store: &MongoSandboxStore,
    permit: &super::execution::FinalizationPermit,
    session: &mut ClientSession,
) -> Result<ShareMutationReceipt, ReadError> {
    apply_inner(store, permit.request(), session, true).await
}

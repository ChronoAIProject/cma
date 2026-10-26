//! Optional envelope evidence. Older writers may drop it; absence always means replay.
use super::{model, record_bson, Record};
use layer2_core::sandbox::{RuntimeBinding, SandboxHome, SandboxStoreError, WarmSandbox};
use layer3_adapter::{
    SetupReceipt, WorkspaceReceipt, WorkspaceRequest, WorkspaceSetupRequest, WorkspaceSource,
};
use mongodb::bson::Document;
use serde::{Deserialize, Serialize};

const FIELD: &str = "workspace_completion_v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativePreparedWorkspaceProof {
    contract_version: u16,
    plan_sha256: String,
    runtime: RuntimeBinding,
    home: Box<SandboxHome>,
    pub(crate) prepare: WorkspaceReceipt,
    pub(crate) setup: Option<SetupReceipt>,
}

pub(crate) fn requests(
    record: &WarmSandbox,
) -> Result<(WorkspaceRequest, WorkspaceSetupRequest), SandboxStoreError> {
    let [workspace] = record.workspaces.as_slice() else {
        return Err(model::corrupt("workspace proof has no exact workspace"));
    };
    let [plan] = record.plan.workspaces.as_slice() else {
        return Err(model::corrupt("workspace proof has no exact plan"));
    };
    if plan.profile.content.workspace_name().ok().as_deref() != Some("workspace")
        || !matches!(&plan.profile.content.location, layer2_core::profiles::WorkspaceLocation::Directory { name } if name == "workspace")
        || !plan.profile.content.preparation_commands().is_empty()
        || !plan.profile.content.credential_commands().is_empty()
    {
        return Err(model::corrupt("workspace proof is outside empty defaults"));
    }
    let execution = record.plan.execution;
    Ok((
        WorkspaceRequest::with_clone_depth(
            format!("workspace:{}:prepare:v1", workspace.id),
            workspace.id.to_string(),
            "workspace",
            WorkspaceSource::Init,
            execution.clone_depth,
        ),
        WorkspaceSetupRequest::with_timeouts(
            format!("workspace:{}:setup:v1", workspace.id),
            workspace.id.to_string(),
            "workspace",
            vec![],
            execution.command_timeout_seconds,
            execution.total_timeout_seconds,
            layer3_adapter::DEFAULT_COMMAND_OUTPUT_LIMIT_BYTES,
        ),
    ))
}

impl NativePreparedWorkspaceProof {
    /// Call only with the actual successful receipt returned by preparation.
    pub(crate) fn prepared(
        record: &WarmSandbox,
        receipt: WorkspaceReceipt,
    ) -> Result<Self, SandboxStoreError> {
        let proof = Self {
            contract_version: 1,
            plan_sha256: model::digest(&record.plan)?,
            runtime: record
                .runtime
                .clone()
                .ok_or_else(|| model::corrupt("workspace proof runtime missing"))?,
            home: record
                .home
                .clone()
                .ok_or_else(|| model::corrupt("workspace proof home missing"))?,
            prepare: receipt,
            setup: None,
        };
        proof.validate(record)?;
        Ok(proof)
    }

    pub(crate) fn completed(
        &self,
        record: &WarmSandbox,
        receipt: SetupReceipt,
    ) -> Result<Self, SandboxStoreError> {
        let mut proof = self.clone();
        proof.setup = Some(receipt);
        proof.validate(record)?;
        Ok(proof)
    }

    pub(crate) fn validate(&self, record: &WarmSandbox) -> Result<(), SandboxStoreError> {
        let (prepare, setup) = requests(record)?;
        let workspace = &record.workspaces[0];
        if self.contract_version != 1
            || self.plan_sha256 != model::digest(&record.plan)?
            || record.runtime.as_ref() != Some(&self.runtime)
            || record.home.as_ref() != Some(&self.home)
            || self.runtime.boot_id.is_none()
            || self.home.released
            || self
                .home
                .native_pool
                .as_ref()
                .is_none_or(|native| native.cleaned || native.claim.is_none())
            || self.prepare.operation_id != prepare.operation_id
            || self.prepare.request_sha256 != prepare.request_sha256
            || self.prepare.workspace_id != prepare.workspace_id
            || self.prepare.name != prepare.name
            || self.prepare.path != "/home/cma/workspace"
            || workspace.resolved_commit.as_ref() != Some(&self.prepare.resolved_commit)
            || !matches!(self.prepare.resolved_commit.len(), 40 | 64)
            || !self
                .prepare
                .resolved_commit
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || self.setup.is_some() != workspace.setup_ready
            || self.setup.as_ref().is_some_and(|receipt| {
                receipt.operation_id != setup.operation_id
                    || receipt.request_sha256 != setup.request_sha256
                    || receipt.workspace_id != setup.workspace_id
                    || !receipt.commands.is_empty()
            })
        {
            return Err(model::corrupt(
                "workspace completion proof does not match preparation",
            ));
        }
        Ok(())
    }
}

pub(super) fn read(
    value: &Document,
    record: &WarmSandbox,
) -> Result<Option<NativePreparedWorkspaceProof>, SandboxStoreError> {
    let Some(value) = value.get(FIELD) else {
        return Ok(None);
    };
    let proof: NativePreparedWorkspaceProof =
        mongodb::bson::deserialize_from_bson(value.clone())
            .map_err(|_| model::corrupt("workspace completion proof is malformed"))?;
    proof.validate(record)?;
    Ok(Some(proof))
}

pub(super) fn document(
    record: &Record,
    proof: Option<&NativePreparedWorkspaceProof>,
) -> Result<Document, SandboxStoreError> {
    let mut value = super::document(record)?;
    if let Some(proof) = proof {
        proof.validate(&record.payload)?;
        value.insert(
            FIELD,
            record_bson(proof).map_err(SandboxStoreError::Corrupt)?,
        );
    }
    Ok(value)
}

pub(super) fn retain(
    value: &mut Document,
    original: &Document,
    record: &Record,
) -> Result<(), SandboxStoreError> {
    if let Some(proof) = read(original, &record.payload)? {
        value.insert(
            FIELD,
            record_bson(&proof).map_err(SandboxStoreError::Corrupt)?,
        );
    }
    Ok(())
}

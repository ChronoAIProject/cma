//! Pinned provider recovery; no command, path, or process identity comes from HTTP.
use std::collections::BTreeSet;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::hosted::{NonexecutingRuntimeProof, RuntimeRecoveryCommand};
use crate::{
    AdapterError, NonexecutingRecoveryVerification, RecoveryConversationHistory, Result,
    RuntimeAdoptionRequest, RuntimeRecovery, RuntimeRecoveryInspection,
    RuntimeRecoveryInspectionRequest, RuntimeRecoveryQuiescence, RuntimeRecoveryQuiescenceRequest,
    SandboxAdapter,
};

use super::CodexAdapter;
use crate::recovery_diagnostics::{Observation, Reason, Stage};

const LIMIT: usize = 64 * 1024;

#[async_trait::async_trait]
impl RuntimeRecovery for CodexAdapter {
    async fn prove_agent_process_closed(
        &self,
        handle: &crate::RuntimeHandle,
        proof: &crate::wire::AgentProcessCloseProof,
    ) -> Result<bool> {
        self.hosted.prove_agent_process_closed(handle, proof).await
    }

    async fn fence_agent_restart(
        &self,
        handle: &crate::RuntimeHandle,
        request: &crate::wire::AgentRestartFence,
    ) -> Result<()> {
        self.hosted.fence_agent_restart(handle, request).await
    }

    async fn inspect_idle_reopen(
        &self,
        request: &RuntimeRecoveryInspectionRequest,
    ) -> Result<RuntimeRecoveryInspection> {
        self.verify_recovery_identity(request).await?;
        let mut input = recovery_input(request)?;
        input["action"] = json!("inspect_idle");
        let output = self.provider_recovery_command(request, &input).await?;
        self.verify_recovery_identity(request).await?;
        checked_inspection(request, &input, output)
    }

    async fn inspect_nonexecuting_recovery(
        &self,
        request: &RuntimeAdoptionRequest,
    ) -> Result<Option<RuntimeRecoveryInspection>> {
        let Some(proof) = self.hosted.inspect_nonexecuting_runtime(request).await? else {
            return Ok(None);
        };
        let provider_plan = serde_json::to_value(proof).map_err(|_| invalid())?;
        Ok(Some(RuntimeRecoveryInspection {
            provider_plan_sha256: plan_digest(&provider_plan)?,
            provider_plan,
            histories: vec![],
            runtime_process_absent: true,
        }))
    }

    async fn verify_nonexecuting_recovery(
        &self,
        request: &NonexecutingRecoveryVerification,
    ) -> Result<()> {
        let proof = self.fresh_nonexecuting_proof(request).await?;
        self.hosted
            .verify_nonexecuting_runtime(&request.source, &proof)
            .await
    }

    async fn suspend_nonexecuting_recovery(
        &self,
        request: &NonexecutingRecoveryVerification,
    ) -> Result<()> {
        let proof = self.fresh_nonexecuting_proof(request).await?;
        self.hosted
            .suspend_nonexecuting_runtime(&request.source, &proof)
            .await
    }

    async fn inspect_recovery(
        &self,
        request: &RuntimeRecoveryInspectionRequest,
    ) -> Result<RuntimeRecoveryInspection> {
        self.verify_recovery_identity(request).await?;
        let input = recovery_input(request)?;
        let output = self.provider_recovery_command(request, &input).await?;
        self.verify_recovery_identity(request).await?;
        Observation::new(Stage::History).adapter(
            checked_inspection(request, &input, output),
            Reason::CorruptHistory,
        )
    }

    async fn quiesce_recovery(
        &self,
        request: &RuntimeRecoveryQuiescenceRequest,
    ) -> Result<RuntimeRecoveryQuiescence> {
        self.verify_recovery_identity(&request.source).await?;
        let mut input = recovery_input(&request.source)?;
        if request.operation_id.is_empty()
            || request.operation_id.len() > 256
            || request.operation_id.chars().any(char::is_control)
            || plan_digest(&request.provider_plan)? != request.provider_plan_sha256
            || request.provider_plan["runtime"] != input["runtime"]
            || request.provider_plan["agents"] != input["agents"]
        {
            return Err(invalid());
        }
        input["action"] = json!("quiesce");
        input["operationId"] = json!(request.operation_id);
        input["expectedPlan"] = request.provider_plan.clone();
        let output = self
            .provider_recovery_command(&request.source, &input)
            .await?;
        self.verify_recovery_identity(&request.source).await?;
        if output["status"] != "quiesced" || output["planSha256"] != request.provider_plan_sha256 {
            return Err(AdapterError::Unavailable(
                "original conversation quiescence is not yet proved; home retained".into(),
            ));
        }
        Ok(RuntimeRecoveryQuiescence {
            histories: checked_histories(&request.source, &output["histories"])?,
        })
    }
}

impl CodexAdapter {
    async fn fresh_nonexecuting_proof(
        &self,
        request: &NonexecutingRecoveryVerification,
    ) -> Result<NonexecutingRuntimeProof> {
        if plan_digest(&request.provider_plan)? != request.provider_plan_sha256 {
            return Err(invalid());
        }
        let admitted: NonexecutingRuntimeProof =
            serde_json::from_value(request.provider_plan.clone()).map_err(|_| invalid())?;
        let fresh = self
            .hosted
            .inspect_nonexecuting_runtime(&request.source)
            .await?
            .ok_or_else(invalid)?;
        if admitted.version != fresh.version
            || admitted.handle != fresh.handle
            || admitted.request_sha256 != fresh.request_sha256
            || admitted.home_identity != fresh.home_identity
        {
            return Err(invalid());
        }
        Ok(fresh)
    }

    async fn verify_recovery_identity(
        &self,
        request: &RuntimeRecoveryInspectionRequest,
    ) -> Result<()> {
        let mut observation = Observation::new(Stage::History);
        let status = observation.adapter(
            self.runtime_status(&request.runtime).await,
            Reason::NativeChanged,
        )?;
        observation.adapter(
            status.validate_for(&request.identity),
            Reason::NativeChanged,
        )?;
        if status.boot_id != request.boot_id {
            observation.finish(Reason::BootMismatch);
            return Err(invalid());
        }
        if status.restore_pending {
            observation.finish(Reason::RestorePending);
            return Err(invalid());
        }
        Ok(())
    }

    async fn provider_recovery_command(
        &self,
        request: &RuntimeRecoveryInspectionRequest,
        input: &Value,
    ) -> Result<Value> {
        let input = serde_json::to_vec(input).map_err(|_| invalid())?;
        if input.len() > LIMIT {
            return Err(invalid());
        }
        let command = RuntimeRecoveryCommand {
            program: "/usr/bin/python3".into(),
            args: vec![
                "-I".into(),
                "-c".into(),
                concat!(
                    include_str!("recovery_io.py"),
                    "\n",
                    include_str!("recovery_ledger.py"),
                    "\n",
                    include_str!("recovery.py")
                )
                .into(),
            ],
            input,
        };
        let mut observation = Observation::new(Stage::History);
        let output = observation.adapter(
            self.hosted
                .run_recovery_command(&request.runtime, &command)
                .await,
            Reason::NativeChanged,
        )?;
        let response: Value = serde_json::from_slice(&output).map_err(|_| {
            observation.finish(Reason::CorruptHistory);
            invalid()
        })?;
        if response["version"] != 1 {
            observation.finish(Reason::CorruptHistory);
            return Err(invalid());
        }
        if response["status"] == "refused" {
            observation.finish(provider_refusal(
                response["code"].as_str().unwrap_or_default(),
            ));
            return Err(invalid());
        }
        Ok(response)
    }
}

fn checked_inspection(
    request: &RuntimeRecoveryInspectionRequest,
    input: &Value,
    output: Value,
) -> Result<RuntimeRecoveryInspection> {
    let plan = output.get("plan").ok_or_else(invalid)?.clone();
    if output["status"] != "inspected"
        || plan["runtime"] != input["runtime"]
        || plan["agents"] != input["agents"]
        || (input["action"] == "inspect_idle"
            && (plan["idleReopen"] != true || !valid_idle_ledger(&plan["idleLedger"])))
    {
        return Err(invalid());
    }
    let digest = plan_digest(&plan)?;
    if output["planSha256"] != digest {
        return Err(invalid());
    }
    let histories = checked_histories(request, &plan["histories"])?;
    Ok(RuntimeRecoveryInspection {
        provider_plan: plan,
        provider_plan_sha256: digest,
        histories,
        // A live supervisor is present. Empty pending Workspace recovery
        // needs separate provider absence evidence and must not use this.
        runtime_process_absent: false,
    })
}

fn valid_idle_ledger(ledger: &Value) -> bool {
    ledger["format"] == "legacy_v1_unused_process_scope"
        && ledger["sha256"].as_str().is_some_and(|hash| hex(hash, 64))
        && ledger["journals"]
            .as_u64()
            .zip(ledger["authorities"].as_u64())
            .is_some_and(|(journals, authorities)| journals <= 128 && authorities <= 128 - journals)
}

fn recovery_input(request: &RuntimeRecoveryInspectionRequest) -> Result<Value> {
    request.identity.validate()?;
    if !hex(&request.boot_id, 32) || request.conversations.len() > 16 {
        return Err(invalid());
    }
    let mut agents = BTreeSet::new();
    for item in &request.conversations {
        if !item
            .agent_id
            .strip_prefix("agt_")
            .is_some_and(|id| hex(id, 32))
            || uuid::Uuid::parse_str(&item.conversation_id).is_err()
            || !agents.insert(&item.agent_id)
        {
            return Err(invalid());
        }
    }
    Ok(json!({
        "version": 1, "action": "inspect",
        "runtime": {
            "sandboxId": request.identity.sandbox_id,
            "homeIdentity": request.identity.home_identity,
            "generation": request.identity.generation,
            "bootId": request.boot_id,
        },
        "agents": request.conversations,
    }))
}

fn plan_digest(plan: &Value) -> Result<String> {
    let bytes = serde_json::to_vec(plan).map_err(|_| invalid())?;
    if plan.as_object().is_none_or(|object| object.is_empty()) || bytes.len() > LIMIT {
        return Err(invalid());
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn checked_histories(
    request: &RuntimeRecoveryInspectionRequest,
    value: &Value,
) -> Result<Vec<RecoveryConversationHistory>> {
    let histories = value.as_array().ok_or_else(invalid)?;
    let expected = request
        .conversations
        .iter()
        .map(|item| (item.agent_id.as_str(), item.conversation_id.as_str()))
        .collect::<BTreeSet<_>>();
    let mut actual = BTreeSet::new();
    let mut result = Vec::new();
    for history in histories {
        let agent = history["agentId"].as_str().ok_or_else(invalid)?;
        let conversation = history["conversationId"].as_str().ok_or_else(invalid)?;
        let hash = history["historySha256"].as_str().ok_or_else(invalid)?;
        if !hex(hash, 64) || !actual.insert((agent, conversation)) {
            return Err(invalid());
        }
        result.push(RecoveryConversationHistory {
            agent_id: agent.into(),
            conversation_id: conversation.into(),
            history_sha256: hash.into(),
        });
    }
    if actual != expected {
        return Err(invalid());
    }
    Ok(result)
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn invalid() -> AdapterError {
    AdapterError::Rejected("original runtime recovery evidence is incomplete or changed".into())
}

#[cfg(test)]
#[path = "recovery_tests.rs"]
mod tests;

fn provider_refusal(code: &str) -> Reason {
    // Exact codes from the pinned recovery.py/recovery_io.py response only.
    // Never infer a category from error text or expose an arbitrary code label.
    match code {
        "deadline" => Reason::Deadline,
        "evidence_unavailable" => Reason::Unavailable,
        "history_missing" => Reason::MissingHistory,
        // history_identity combines absent, malformed and duplicate identity.
        // Keep the shared incomplete/unverifiable-history category truthful.
        "history_incomplete"
        | "history_invalid"
        | "history_prefix_changed"
        | "history_replaced"
        | "history_identity"
        | "history_descriptor"
        | "history_symlink"
        | "history_path"
        | "history_limit"
        | "turn_identity"
        | "turn_limit"
        | "native_turn_unsettled"
        | "native_turn_restarted" => Reason::CorruptHistory,
        "boot_changed" => Reason::BootMismatch,
        "home_changed" => Reason::HomeChanged,
        "ledger_format_unproved" => Reason::LedgerFormatUnproved,
        "ledger_capacity"
        | "ledger_unproved"
        | "ledger_unsettled"
        | "ledger_process_scope_used" => Reason::LedgerHeld,
        "ledger_changed" => Reason::NativeChanged,
        "supervisor_changed" | "pid_reused" | "untracked_process" | "process_reappeared" => {
            Reason::NativeChanged
        }
        // Request/unsupported-topology and shared path/journal guards are not
        // necessarily history failures. Unknown codes retain this fixed refusal.
        _ => Reason::Ineligible,
    }
}

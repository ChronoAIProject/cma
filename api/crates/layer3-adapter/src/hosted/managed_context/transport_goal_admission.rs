use super::HostedContextRefresh;
use crate::hosted::{retained_json::JsonFrame, RuntimeRecoveryCommand};
use crate::ingress_budget::{
    AdmissionRefusal, AdmittedError, AdmittedOperation, AllocationPlan, IngressClass,
    IngressOwnershipPolicy,
};

const SCRIPT: &str = include_str!("transport_goal.py");

#[derive(serde::Serialize)]
struct GoalRequest<'a> {
    home: &'a str,
    session: &'a str,
}

fn invalid() -> AdmissionRefusal {
    AdmissionRefusal::InvalidPlan {
        phase: "retained goal inspection",
    }
}

impl HostedContextRefresh {
    pub(in crate::hosted) fn transport_goal_absent_admitted<'a>(
        &'a self,
        session: &'a str,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, bool>, AdmittedError> {
        let home = self.0.path.strip_suffix("/AGENTS.md").ok_or_else(invalid)?;
        let home_bytes = home.len().checked_add("/home/".len()).ok_or_else(invalid)?;
        // JSON can escape each UTF-8 input byte to six ASCII bytes. Keys,
        // quotes, braces and separators add exactly 24 bytes to this object.
        let input_bytes = home_bytes
            .checked_add(session.len())
            .and_then(|n| n.checked_mul(6))
            .and_then(|n| n.checked_add(24))
            .ok_or_else(invalid)?;
        let owned_policy = policy.clone();
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Data,
            AllocationPlan::checked(
                "retained goal command",
                &[
                    home_bytes,
                    input_bytes,
                    "/usr/bin/python3".len(),
                    3 * size_of::<String>(),
                    "-I-c".len(),
                    SCRIPT.len(),
                ],
            )?,
            move |scope| async move {
                let policy = owned_policy;
                let before = self.ready_status_admitted(&policy)?.await?;
                if before.is_none() {
                    return Ok(scope.initialize_owned(|| false));
                }
                let mut full_home = String::with_capacity(home_bytes);
                full_home.push_str("/home/");
                full_home.push_str(home);
                let mut input = Vec::with_capacity(input_bytes);
                serde_json::to_writer(
                    &mut input,
                    &GoalRequest {
                        home: &full_home,
                        session,
                    },
                )
                .map_err(|_| invalid())?;
                let command = scope.initialize_owned(|| RuntimeRecoveryCommand {
                    program: "/usr/bin/python3".into(),
                    args: vec!["-I".into(), "-c".into(), SCRIPT.into()],
                    input,
                });
                let source = self.0.backend.run_recovery_command_admitted(
                    &self.0.handle,
                    command,
                    &policy,
                )?;
                if !source.belongs_to(&policy) {
                    return Err(invalid().into());
                }
                let output = source.await?;
                if !output.belongs_to(&policy) {
                    return Err(invalid().into());
                }
                let result = JsonFrame::decode_admitted(&output, &policy)?;
                let valid = result.as_object().is_some_and(|fields| fields.len() == 3)
                    && result.get("version") == Some(&serde_json::Value::from(1))
                    && result.get("session").and_then(serde_json::Value::as_str) == Some(session)
                    && result.get("goal_absent") == Some(&serde_json::Value::Bool(true));
                if !valid {
                    return Ok(scope.initialize_owned(|| false));
                }
                let after = self.ready_status_admitted(&policy)?.await?;
                Ok(scope.initialize_owned(|| *after == *before))
            },
        )
    }
}

#[cfg(test)]
#[path = "transport_goal_admission_tests.rs"]
mod tests;

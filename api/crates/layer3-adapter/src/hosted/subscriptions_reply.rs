use super::{AdmittedSubscriptionBinding, HostedSubscriptionCredential};
use crate::hosted::{acp::Bridge, retained_json::JsonFrame};
use crate::ingress_budget::{
    AdmissionRefusal, AdmittedError, AdmittedOperation, AllocationPlan, IngressClass,
    IngressOwnershipPolicy,
};
use serde::Serialize;
use serde_json::Value;

#[derive(Serialize)]
struct Reply<'a> {
    jsonrpc: &'static str,
    id: &'a Value,
    result: Credential<'a>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Credential<'a> {
    Warm {
        mode: &'static str,
    },
    Connected {
        mode: &'static str,
        tokens: Tokens<'a>,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Tokens<'a> {
    access_token: &'a str,
    chatgpt_account_id: &'a str,
    chatgpt_plan_type: Option<&'a str>,
}

fn invalid(phase: &'static str) -> AdmittedError {
    AdmissionRefusal::InvalidPlan { phase }.into()
}

impl AdmittedSubscriptionBinding {
    pub(in crate::hosted) fn reply_admitted<'a>(
        &'a self,
        id: &'a Value,
        params: &'a Value,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, JsonFrame>, AdmittedError> {
        if !self.belongs_to(policy) {
            return Err(invalid("subscription binding ownership"));
        }
        if !params.is_object() || !crate::hosted::acp_correlation::valid_call_with_policy(id, true)
        {
            return Err(invalid("subscription request boundary"));
        }
        let refresh = params
            .get("forceRefresh")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let previous = params.get("previousAccountId").and_then(Value::as_str);
        if previous.is_some_and(|account| account.is_empty() || account.len() > 256) {
            return Err(invalid("subscription account boundary"));
        }
        let owned_policy = policy.clone();
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Data,
            AllocationPlan::checked("subscription reply operation", &[0])?,
            move |scope| async move {
                let binding = self.value();
                let source = binding.source.credential_admitted(
                    &binding.sandbox_id,
                    &binding.runtime,
                    &binding.agent,
                    refresh,
                    previous,
                    &owned_policy,
                )?;
                if !source.belongs_to(&owned_policy) {
                    return Err(invalid("subscription source ownership"));
                }
                let credential = source.await?;
                if !credential.belongs_to(&owned_policy) {
                    return Err(invalid("subscription result ownership"));
                }
                let result = match &*credential {
                    HostedSubscriptionCredential::Warm if !refresh && previous.is_none() => {
                        Credential::Warm { mode: "warm" }
                    }
                    HostedSubscriptionCredential::Warm => {
                        return Err(invalid("warm subscription refresh"))
                    }
                    HostedSubscriptionCredential::Connected {
                        access_token,
                        account_id,
                        plan_type,
                    } => Credential::Connected {
                        mode: "subscription",
                        tokens: Tokens {
                            access_token: access_token.expose(),
                            chatgpt_account_id: account_id,
                            chatgpt_plan_type: plan_type.as_deref(),
                        },
                    },
                };
                // Borrow the credential through the allocating serialization. The
                // completed JSON tree receives independent admission and custody.
                let reply = Reply {
                    jsonrpc: "2.0",
                    id,
                    result,
                };
                let prepared = scope
                    .initialize_owned(|| async { JsonFrame::prepare(&reply, Some(&owned_policy)) })
                    .resolve_admitted_result(&owned_policy, IngressClass::Data)
                    .await;
                prepared
            },
        )
    }
}

impl Bridge {
    pub(in crate::hosted) async fn subscription_credential_admitted(
        &self,
        id: &Value,
        params: Option<&Value>,
        policy: &IngressOwnershipPolicy,
    ) {
        let operation = AdmittedOperation::new(
            policy,
            IngressClass::Control,
            AllocationPlan::checked("subscription credential handler", &[0])
                .expect("zero working plan"),
            |_| async {
                let reply = match (&self.subscription, params) {
                    (Some(binding), Some(params)) => {
                        match binding.reply_admitted(id, params, policy) {
                            Ok(operation) => operation.await.ok(),
                            Err(_) => None,
                        }
                    }
                    _ => None,
                };
                if let Some(reply) = reply {
                    self.send(&reply).await
                } else {
                    // Every source failure keeps the existing redacted protocol
                    // refusal, including refusal before any credential allocation.
                    self.control_reply(id, || {
                        serde_json::json!({"jsonrpc":"2.0", "id":id, "error":{
                            "code":-32001, "message":"Re-login with Codex subscription to continue"
                        }})
                    })
                    .await
                }
            },
        );
        let sent = match operation {
            Ok(operation) => operation.await.map(|_| ()),
            Err(error) => Err(error),
        };
        if sent.is_err() {
            tracing::debug!("subscription credential reply could not be delivered");
        }
    }
}

#[cfg(test)]
#[path = "subscriptions_reply_tests.rs"]
mod tests;

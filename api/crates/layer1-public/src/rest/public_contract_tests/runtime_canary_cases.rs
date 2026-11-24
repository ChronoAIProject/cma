use std::sync::Arc;

use axum::Extension;
use layer2_core::{resource_access::CallerContext, Authorization, Capability};

use super::Scenario;
use crate::rest::AgentLifecycleCreateReceipt;
use crate::rest::{retained_runtime_transition as retained, runtime_canary::*};

pub(super) fn scenario(name: &str) -> Option<Scenario> {
    let retained = name.starts_with("retained_runtime_");
    let prefix = if retained {
        "retained_runtime_"
    } else {
        "runtime_canary_"
    };
    let (caller, outcome) = name.strip_prefix(prefix)?.split_once(':')?;
    let authorization = match caller {
        "operator" => Authorization::new([Capability::ConfigurePlatform]),
        "owner" | "delegated" => {
            Authorization::new([Capability::UseOwnSandboxes, Capability::CreateSandbox])
        }
        "guest" | "anonymous" => Authorization::none(),
        _ => return None,
    };
    let error = match outcome {
        "ok" => None,
        "conflict" => Some(CanaryRefusal::Conflict),
        "closed" => Some(CanaryRefusal::Closed),
        "unavailable" => Some(CanaryRefusal::Unavailable),
        _ => return None,
    };
    let subject = (caller != "anonymous").then_some("fixture-user");
    let mut router = if retained {
        retained::router(Arc::new(Service(error)))
    } else {
        runtime_canary_router(Arc::new(Service(error)))
    };
    if let Some(subject) = subject {
        use layer2_core::resource_access::{CredentialSource, ResourceActor};
        let mut context = crate::rest::resource_access_tests::context(subject);
        context.caller.authorization = authorization.clone();
        if caller == "delegated" {
            context.credential_source = CredentialSource::delegated("fixture-grant");
            context.actor = ResourceActor::Api {
                subject: context.caller.subject.clone(),
                delegation_id: Some("fixture-grant".to_owned()),
            };
        }
        router = router.layer(Extension(context));
    }
    Some(Scenario {
        router,
        subject,
        authorization,
        symbols: Default::default(),
    })
}

struct Service(Option<CanaryRefusal>);

#[async_trait::async_trait]
impl RuntimeCanaryService for Service {
    async fn configure(&self, _: CanaryPolicyRequest) -> Result<CanaryPolicyView, CanaryRefusal> {
        self.result(CanaryPolicyView { revision: 2 })
    }

    async fn grant(&self, _: CanaryGrantRequest) -> Result<CanaryGrantView, CanaryRefusal> {
        self.result(CanaryGrantView {
            grant_id: "fixture-grant".into(),
            revision: 1,
            policy_revision: 2,
        })
    }

    async fn revoke(&self, _: CanaryRevokeRequest) -> Result<CanaryGrantView, CanaryRefusal> {
        self.result(CanaryGrantView {
            grant_id: "fixture-grant".into(),
            revision: 2,
            policy_revision: 2,
        })
    }

    async fn redeem(
        &self,
        context: &CallerContext,
        key: &str,
        _: CanaryRedeemRequest,
    ) -> Result<AgentLifecycleCreateReceipt, CanaryRefusal> {
        assert_eq!(context.caller.subject.as_str(), "fixture-user");
        assert_eq!(key, "fixture-redemption");
        self.result(AgentLifecycleCreateReceipt {
            id: "fixture-operation".into(),
            status: "provisioning".into(),
        })
    }
}

impl Service {
    fn result<T>(&self, value: T) -> Result<T, CanaryRefusal> {
        match self.0 {
            Some(CanaryRefusal::Conflict) => Err(CanaryRefusal::Conflict),
            Some(CanaryRefusal::Closed) => Err(CanaryRefusal::Closed),
            Some(CanaryRefusal::Unavailable) => Err(CanaryRefusal::Unavailable),
            None => Ok(value),
            _ => unreachable!("fixture defines only state-dependent refusals"),
        }
    }
}

#[async_trait::async_trait]
impl retained::RetainedRuntimeTransitions for Service {
    async fn configure(&self, _: CanaryPolicyRequest) -> Result<CanaryPolicyView, CanaryRefusal> {
        self.result(CanaryPolicyView { revision: 2 })
    }
    async fn preview(
        &self,
        _: retained::RetainedRuntimeProposal,
    ) -> Result<retained::RetainedRuntimePreview, CanaryRefusal> {
        self.result(retained::RetainedRuntimePreview {
            predecessor_generation: 1,
            predecessor_image: "registry.example/runtime@sha256:".to_owned() + &"a".repeat(64),
            predecessor_journal_recovery: false,
            target_image: "registry.example/runtime@sha256:".to_owned() + &"a".repeat(64),
            target_journal_recovery: true,
            plan_sha256: "b".repeat(64),
        })
    }
    async fn grant(
        &self,
        operator: &layer2_core::SubjectId,
        _: retained::RetainedRuntimeGrantRequest,
    ) -> Result<retained::RetainedRuntimeGrantView, CanaryRefusal> {
        assert_eq!(operator.as_str(), "fixture-user");
        self.result(retained::RetainedRuntimeGrantView {
            grant_id: "fixture-grant".into(),
        })
    }
    async fn abort(
        &self,
        _: retained::RetainedRuntimeAbort,
    ) -> Result<retained::RetainedRuntimeGrantView, CanaryRefusal> {
        self.result(retained::RetainedRuntimeGrantView {
            grant_id: "fixture-grant".into(),
        })
    }
    async fn redeem(
        &self,
        owner: &layer2_core::SubjectId,
        _: retained::RetainedRuntimeRedemption,
    ) -> Result<retained::RetainedRuntimeReceipt, CanaryRefusal> {
        assert_eq!(owner.as_str(), "fixture-user");
        self.result(retained::RetainedRuntimeReceipt {
            operation_id: "fixture-operation".into(),
            successor_generation: 2,
        })
    }
}

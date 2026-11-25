use super::{model::*, BindingPreparation, PreparationIntent, PreparationReceipt};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::routing_store::{AdmittedPlan, StoredTarget},
};
use cmaeg_core::channel::{ProviderBinding, Secret};
use futures_util::future::BoxFuture;

/// A direct, awaited transport: it must not spawn a detached start or re-render
/// any frozen field. Exact-body idempotency is opt-in only after provider proof.
pub trait ExecutionPort: Send + Sync {
    fn support(&self, binding: &ProviderBinding) -> RecoverySupport;
    /// Preparation may read/render, but must never start a provider turn.
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>>;
    fn inspect_binding<'a>(
        &'a self,
        _plan: &'a AdmittedPlan,
        _target: &'a StoredTarget,
        _current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<BindingPreparation, ExecutionError>> {
        Box::pin(async { Err(ExecutionError::Unsupported) })
    }
    fn reserve_binding<'a>(
        &'a self,
        _target: &'a StoredTarget,
        _current: &'a ResolvedChannel,
        _intent: &'a PreparationIntent,
    ) -> BoxFuture<'a, Result<PreparationReceipt, ExecutionError>> {
        Box::pin(async { Err(ExecutionError::Unsupported) })
    }
    fn prepare_at_generation<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
        generation: Option<u64>,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        if generation.is_some() {
            Box::pin(async { Err(ExecutionError::Unsupported) })
        } else {
            self.prepare(plan, target, current)
        }
    }
    fn start<'a>(
        &'a self,
        body: &'a FrozenTurn,
        binding: &'a ProviderBinding,
        credential: &'a Secret,
    ) -> BoxFuture<'a, StartResult>;
    fn start_observed<'a>(
        &'a self,
        body: &'a FrozenTurn,
        binding: &'a ProviderBinding,
        credential: &'a Secret,
        _timing: Option<crate::telemetry::timing::IngressTiming>,
    ) -> BoxFuture<'a, StartResult> {
        self.start(body, binding, credential)
    }
    /// Persist independent continuation scheduling after immutable body freeze.
    /// Failure must prevent the original provider POST; a wake-up never grants
    /// permission to replay that original request.
    fn prepare_continuations<'a>(
        &'a self,
        _plan: &'a AdmittedPlan,
        _target: &'a StoredTarget,
        _body: &'a FrozenTurn,
    ) -> BoxFuture<'a, Result<(), ExecutionError>> {
        Box::pin(async { Ok(()) })
    }
}

pub enum StartResult {
    Accepted {
        response_id: String,
    },
    /// Proved no work was accepted, so the exact request can be retried.
    NotAccepted,
    NotAcceptedWithCode {
        code: String,
    },
    /// A terminal rejection proved by the provider, never a network/5xx guess.
    Rejected {
        code: String,
    },
    Unknown,
}

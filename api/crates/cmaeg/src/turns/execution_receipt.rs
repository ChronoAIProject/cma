//! Provider adapters certify execution on terminal evidence. The Responses
//! protocol alone makes no claim about the executor or handoff.

use crate::{
    activity::{Change, ExecutionAttribution},
    ingress::launch::LaunchRequest,
    providers::ResponsesEvent,
};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionScope {
    pub provider: String,
    pub channel_record_id: String,
    pub event_id: String,
    pub consumer_id: String,
    pub response_id: String,
}

/// The adapter certifies that the terminal receipt resolves to the executor
/// for this response and that the reference is safe to display to the owner.
pub struct ExecutionReceipt {
    pub scope: ExecutionScope,
    pub reference: String,
}

pub trait ExecutionReceiptVerifier: Send + Sync {
    /// Verify evidence from this provider's authenticated terminal response.
    /// Missing evidence, multiple possible executors and unverifiable
    /// handoffs must return `None`.
    fn verify(&self, scope: &ExecutionScope, terminal: &Value) -> Option<ExecutionReceipt>;
}

#[derive(Default)]
pub struct ExecutionVerifiers {
    enabled: bool,
    providers: BTreeMap<String, Arc<dyn ExecutionReceiptVerifier>>,
}

impl ExecutionVerifiers {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            providers: BTreeMap::new(),
        }
    }

    pub fn with_provider(
        mut self,
        provider: cmaeg_core::ids::ProviderSlug,
        verifier: Arc<dyn ExecutionReceiptVerifier>,
    ) -> Self {
        if self.enabled {
            self.providers.insert(provider.to_string(), verifier);
        }
        self
    }

    pub(crate) fn verifier(&self, provider: &str) -> Option<&dyn ExecutionReceiptVerifier> {
        self.providers.get(provider).map(Arc::as_ref)
    }
}

pub(super) fn observe(
    verifier: Option<&dyn ExecutionReceiptVerifier>,
    turn: &LaunchRequest,
    provider: &str,
    admitted_response: Option<&str>,
    event: &ResponsesEvent,
) -> Option<Change> {
    let verifier = verifier?;
    let admitted_response = admitted_response?;
    let scope = ExecutionScope {
        provider: provider.into(),
        channel_record_id: turn.channel.definition.record_id.to_string(),
        event_id: turn.activity.event_id.to_string(),
        consumer_id: turn
            .consumer
            .as_ref()
            .map_or("default", |id| id.as_str())
            .into(),
        response_id: admitted_response.into(),
    };
    verified_change(verifier, &scope, event)
}

pub(super) fn verified_change(
    verifier: &dyn ExecutionReceiptVerifier,
    scope: &ExecutionScope,
    event: &ResponsesEvent,
) -> Option<Change> {
    let ResponsesEvent::Completed {
        response,
        matching_response_identity: true,
    } = event
    else {
        return None;
    };
    if response.get("id")?.as_str()? != scope.response_id {
        return None;
    }
    // The routed frame hash excludes negotiated trace annotations.
    // A verifier may only use evidence covered by execution identity.
    let mut without_trace;
    let response = if response.get("nyxid_trace").is_some() {
        without_trace = response.clone();
        without_trace.as_object_mut()?.remove("nyxid_trace");
        &without_trace
    } else {
        response
    };
    let receipt = verifier.verify(scope, response)?;
    if receipt.scope != *scope {
        return None;
    }
    let attribution = ExecutionAttribution::new(
        scope.provider.clone(),
        scope.response_id.clone(),
        receipt.reference,
    )?;
    Some(Change::execution_attribution(
        &scope.consumer_id,
        attribution,
    ))
}

#[cfg(test)]
#[path = "execution_receipt_tests.rs"]
mod tests;

//! A worker adopts only a validated, retained provider association.
use super::{runtime, scope, Operation, Outcome, Stage};
use std::future::Future;
use tracing::{Instrument, Span};

pub(crate) struct BoundOperation {
    operation: Operation,
    span: Span,
}

impl BoundOperation {
    pub(crate) fn child_of(parent: &Span, stage: Stage) -> Option<Self> {
        let writer = runtime::writer()?;
        let parent = scope::context(parent)?;
        Some(Self::from_operation(Operation::start(
            writer,
            stage,
            Some(parent),
            None,
        )))
    }

    pub(crate) fn from_operation(operation: Operation) -> Self {
        let span = scope::span();
        scope::attach(&span, operation.context());
        Self { operation, span }
    }

    pub(crate) async fn within<T>(&self, future: impl Future<Output = T>) -> T {
        future.instrument(self.span.clone()).await
    }

    pub(crate) fn finish(self, outcome: Outcome) {
        self.operation.finish(outcome, None);
    }
}

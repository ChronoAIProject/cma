//! Attach independently verified identity to an admitted HTTP audit attempt.
use std::{future::Future, sync::Arc};

use layer2_core::{ActivityChannel, StoreError, SubjectId};

tokio::task_local! {
    static CURRENT: Arc<dyn Attempt>;
}

#[derive(Clone, Copy)]
pub enum Credential {
    WebSession,
    DirectBearer,
    AgentKey,
    BindingKey,
    IdentityProxy,
    RequestBoundRead,
    Service,
    Other,
}

impl Credential {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WebSession => "web_session",
            Self::DirectBearer => "bearer",
            Self::AgentKey => "agent_key",
            Self::BindingKey => "binding_key",
            Self::IdentityProxy => "identity_proxy",
            Self::RequestBoundRead => "request_bound_read",
            Self::Service => "service",
            Self::Other => "other",
        }
    }
}

pub struct VerifiedActor {
    pub subject: SubjectId,
    pub channel: ActivityChannel,
    pub credential: Credential,
    pub scope: &'static str,
}

#[async_trait::async_trait]
pub trait Attempt: Send + Sync {
    async fn bind(&self, actor: VerifiedActor) -> Result<(), StoreError>;
}

pub async fn scope<F: Future>(attempt: Arc<dyn Attempt>, future: F) -> F::Output {
    CURRENT.scope(attempt, future).await
}

pub async fn bind(actor: VerifiedActor) -> Result<(), StoreError> {
    match CURRENT.try_with(Arc::clone) {
        Ok(attempt) => attempt.bind(actor).await,
        Err(_) => Ok(()),
    }
}

pub fn active() -> bool {
    CURRENT.try_with(|_| ()).is_ok()
}

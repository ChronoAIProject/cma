//! Shared service composition; every builder preserves optional read ports.
use super::*;

impl AgentReadService {
    pub fn new<S, R>(store: Arc<S>, resources: Arc<R>) -> Arc<Self>
    where
        S: AgentReadStore + 'static,
        R: SandboxResourceReader + 'static,
    {
        Arc::new(Self {
            store,
            sessions: None,
            resources,
            access: None,
            response_logs: None,
        })
    }

    pub fn with_resource_access(self: &Arc<Self>, access: Arc<ResourceAccessService>) -> Arc<Self> {
        Arc::new(Self {
            store: self.store.clone(),
            sessions: self.sessions.clone(),
            resources: self.resources.clone(),
            access: Some(access),
            response_logs: self.response_logs.clone(),
        })
    }

    pub fn with_sessions(
        self: &Arc<Self>,
        sessions: Arc<dyn super::store::SandboxSessionStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store: self.store.clone(),
            sessions: Some(sessions),
            resources: self.resources.clone(),
            access: self.access.clone(),
            response_logs: self.response_logs.clone(),
        })
    }
}

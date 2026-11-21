use super::AgentRuntimeTarget;

pub(super) struct RefreshRecoveryOnDrop {
    pub(super) runtimes: std::sync::Arc<dyn super::super::AgentRuntimeRegistry>,
    pub(super) target: AgentRuntimeTarget,
    pub(super) armed: bool,
}
impl Drop for RefreshRecoveryOnDrop {
    fn drop(&mut self) {
        if self.armed {
            let runtimes = self.runtimes.clone();
            let target = self.target.clone();
            tokio::spawn(async move {
                let _ = runtimes.request_reconnect(&target).await;
            });
        }
    }
}

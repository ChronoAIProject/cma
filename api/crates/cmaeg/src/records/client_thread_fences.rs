//! Optional persistent thread storage requires the Gateway database.
use super::{RecordClient, RecordError};
use crate::records::RecordKind;
use std::time::Duration;

impl RecordClient {
    /// Permanent exact-thread write fences never share an expiring record domain.
    pub(crate) fn thread_fences(mut self) -> Self {
        self.record_path = "/api/v1/gateway/thread-fence-records";
        self.record_kind = RecordKind::ThreadFence;
        self
    }

    pub(crate) async fn thread_fences_ready(&self) -> bool {
        #[cfg(test)]
        if self.thread_fences_fixture && self.database.is_none() {
            return true;
        }
        let Some(database) = &self.database else {
            return false;
        };
        tokio::time::timeout(Duration::from_secs(2), database.thread_fences_ready())
            .await
            .is_ok_and(|result| result.is_ok())
    }

    #[cfg(test)]
    pub(crate) fn with_thread_fences_fixture(mut self) -> Self {
        self.thread_fences_fixture = true;
        self
    }

    pub(super) fn check_thread_fence_transport(&self) -> Result<(), RecordError> {
        if self.record_kind == RecordKind::ThreadFence && self.database.is_none() {
            #[cfg(test)]
            let fixture = self.thread_fences_fixture;
            #[cfg(not(test))]
            let fixture = false;
            if !fixture {
                return Err(RecordError::Unavailable(
                    "persistent thread fences require Gateway Mongo storage".into(),
                ));
            }
        }
        Ok(())
    }
}

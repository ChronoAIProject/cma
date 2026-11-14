//! A call-local barrier between independent reads and runtime authority.
use tokio::sync::watch;

use crate::{AdapterError, Result};

/// Only read preparation may precede this barrier. No result may be cached or
/// shared with another inspection, and dropping the release refuses admission.
pub struct InspectionAdmission(Option<watch::Receiver<bool>>);

pub struct InspectionRelease(watch::Sender<bool>);

impl InspectionAdmission {
    pub fn pending() -> (InspectionRelease, Self) {
        let (release, admission) = watch::channel(false);
        (InspectionRelease(release), Self(Some(admission)))
    }

    pub fn ready() -> Self {
        Self(None)
    }

    pub(crate) fn is_ready(&self) -> bool {
        self.0.as_ref().is_none_or(|receiver| *receiver.borrow())
    }

    pub async fn wait(&self) -> Result<()> {
        let Some(mut receiver) = self.0.clone() else {
            return Ok(());
        };
        receiver
            .wait_for(|ready| *ready)
            .await
            .map(|_| ())
            .map_err(|_| AdapterError::Rejected("runtime inspection was not admitted".into()))
    }
}

impl InspectionRelease {
    pub fn allow(self) {
        // The reader can already have failed or been cancelled. Admission does
        // not resurrect it or authorize a replacement operation.
        self.0.send_replace(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_explicit_ready_admission_never_waits() {
        let admission = InspectionAdmission::ready();
        assert!(admission.is_ready());
        admission.wait().await.unwrap();
    }

    #[tokio::test]
    async fn released_admission_survives_its_consumed_sender() {
        let (release, admission) = InspectionAdmission::pending();
        assert!(!admission.is_ready());
        release.allow();
        assert!(admission.is_ready());
        admission.wait().await.unwrap();
        admission.wait().await.unwrap();
    }

    #[tokio::test]
    async fn abandoned_admission_refuses_every_waiter() {
        let (release, admission) = InspectionAdmission::pending();
        drop(release);
        assert!(!admission.is_ready());
        assert!(admission.wait().await.is_err());
        assert!(admission.wait().await.is_err());
    }

    #[test]
    fn releasing_an_already_cancelled_reader_has_no_replacement_effect() {
        let (release, admission) = InspectionAdmission::pending();
        drop(admission);
        release.allow();
    }
}

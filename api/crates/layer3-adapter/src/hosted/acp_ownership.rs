//! A bridge captures its opening epoch; later callers cannot replace it.
use super::{Bridge, Message};
use crate::AdapterError;
use futures_util::SinkExt as _;

impl Bridge {
    pub(super) async fn check_owner(&self) -> crate::Result<()> {
        match &self.authority {
            Some(authority) if !authority.is_current() => {
                Err(AdapterError::Rejected("conversation owner expired".into()))
            }
            Some(_) => Ok(()),
            None => Ok(()),
        }
    }

    pub(super) async fn owner_revoked(&self) {
        match &self.authority {
            Some(authority) => authority.revoked().await,
            None => std::future::pending().await,
        }
    }

    pub(super) async fn write_owned(&self, message: Message) -> crate::Result<()> {
        let write = async {
            let mut sink = self.sink.lock().await;
            // Recheck after queueing behind another sender, including ACKs.
            self.check_owner().await?;
            sink.send(message)
                .await
                .map_err(|_| AdapterError::Unavailable("conversation write failed".into()))
        };
        if self.authority.is_none() {
            return write.await;
        }
        tokio::select! { biased;
            _ = self.owner_revoked() => Err(AdapterError::Rejected("conversation owner expired".into())),
            result = tokio::time::timeout(std::time::Duration::from_secs(5), write) => result
                .map_err(|_| AdapterError::Unavailable("owned conversation write timed out".into()))?,
        }
    }
}

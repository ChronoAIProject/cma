use super::{DesktopConnection, DesktopError, DesktopFrame};
use crate::sandbox::ingress_ownership::AdmittedFrame;

impl DesktopConnection {
    /// Forward allocation custody to the transport without copying the payload.
    /// A transport without an admitted sink explicitly refuses this operation.
    pub async fn send_admitted(
        &mut self,
        frame: AdmittedFrame<DesktopFrame>,
    ) -> Result<(), DesktopError> {
        let connection = self.connection.as_mut().ok_or(DesktopError::Unavailable)?;
        if let Some(owned) = &mut self.owned {
            return owned.send_admitted(connection, frame).await;
        }
        connection
            .send_admitted(frame)
            .await
            .map_err(DesktopError::adapter)
    }

    /// Keep the producer's allocation owner attached through the final consumer.
    /// This never falls back to an unowned receive followed by admission.
    pub async fn recv_admitted(
        &mut self,
    ) -> Result<Option<AdmittedFrame<DesktopFrame>>, DesktopError> {
        let connection = self.connection.as_mut().ok_or(DesktopError::Unavailable)?;
        if let Some(owned) = &mut self.owned {
            return owned.recv_admitted(connection).await;
        }
        connection
            .recv_admitted()
            .await
            .map_err(DesktopError::adapter)
    }
}

//! RPC response deadlines advance only when startup output is durably acknowledged.
use std::time::Duration;

use super::{Bridge, CALL_DEADLINE, SIGN_IN_DEADLINE};
use crate::AdapterError;

impl Bridge {
    pub(in crate::hosted) async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> crate::Result<serde_json::Value> {
        self.call_within(method, params, CALL_DEADLINE).await
    }

    pub(in crate::hosted) async fn call_within(
        &self,
        method: &str,
        params: serde_json::Value,
        deadline: Duration,
    ) -> crate::Result<serde_json::Value> {
        let progress = if matches!(method, "initialize" | "session/load") {
            self.output_progress().await
        } else {
            None
        };
        let mut waiting = self.begin(method, params).await?;
        let waiting = async {
            let lost = |_| AdapterError::Unavailable(format!("{method} lost its answer"));
            let Some(mut progress) = progress else {
                return tokio::time::timeout(deadline, waiting)
                    .await
                    .map_err(|_| AdapterError::Unavailable(format!("{method} was never answered")))?
                    .map_err(lost);
            };
            let idle = tokio::time::sleep(deadline);
            tokio::pin!(idle);
            loop {
                tokio::select! {
                    result = &mut waiting => return result.map_err(lost),
                    _ = &mut idle => return Err(AdapterError::Unavailable(format!("{method} made no durable output progress"))),
                    result = progress.changed() => {
                        if result.is_err() { return waiting.await.map_err(lost); }
                        idle.as_mut().reset(tokio::time::Instant::now() + deadline);
                    }
                }
            }
        };
        // Progress is durable ACK progress, never merely producer activity. An
        // absolute bound still limits a peer that keeps emitting output forever.
        let frame = tokio::time::timeout(SIGN_IN_DEADLINE, waiting)
            .await
            .map_err(|_| {
                AdapterError::Unavailable(format!("{method} exceeded its recovery deadline"))
            })??;
        if let Some(error) = frame.get("error") {
            return Err(AdapterError::Rejected(format!("{method}: {error}")));
        }
        Ok(frame
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }
}

use super::*;

impl Bridge {
    pub(in crate::hosted) async fn send(&self, frame: &serde_json::Value) -> crate::Result<()> {
        self.send_frame(frame, false).await
    }

    pub(super) async fn send_frame(
        &self,
        frame: &serde_json::Value,
        prompt: bool,
    ) -> crate::Result<()> {
        if self.retired.load(Ordering::SeqCst) {
            return Err(retired_before_write(prompt));
        }
        let body = serde_json::to_vec(frame)
            .map_err(|_| AdapterError::Rejected("ACP frame could not be encoded".into()))?;
        if body.len() > MAX_FRAME_BYTES {
            return Err(AdapterError::Rejected(
                "ACP frame exceeds the size limit".into(),
            ));
        }
        let mut framed = Vec::with_capacity(body.len() + 1);
        framed.push(LANE_CONVERSATION);
        framed.extend_from_slice(&body);
        self.sink
            .lock()
            .await
            .send(Message::Binary(framed))
            .await
            .map_err(|error| {
                AdapterError::Unavailable(format!("conversation write failed: {error}"))
            })
    }

    pub(in crate::hosted) async fn retire(&self) {
        if !self.retired.swap(true, Ordering::SeqCst) {
            self.pending.lock().await.clear();
            self.blocked.lock().await.clear();
            let _ = self.sink.lock().await.close().await;
        }
    }

    pub(super) async fn finish(&self) {
        self.retired.store(true, Ordering::SeqCst);
        self.pending.lock().await.clear();
        self.blocked.lock().await.clear();
    }
}

pub(super) fn retired_before_write(prompt: bool) -> AdapterError {
    if prompt {
        AdapterError::PromptNotWritten("conversation link has retired".into())
    } else {
        AdapterError::Unavailable("conversation link has retired".into())
    }
}

use super::*;
use layer3_adapter::{AdapterError, RuntimeOutputReceipt, RuntimeOutputSource};
use tokio::sync::mpsc;

#[derive(Default)]
pub(in super::super) struct StartupOutput {
    pub cursors: Mutex<Vec<u64>>,
    pub submitted: Mutex<Vec<u64>>,
    pub acknowledged: Mutex<Vec<u64>>,
}

impl NativeBackend {
    pub(super) async fn before_reopen(
        &self,
        native: &str,
        updates: &mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<()> {
        if self.hold_reopen.swap(false, Ordering::SeqCst) {
            self.reopen_entered.notify_one();
            self.reopen_release.notified().await;
        }
        if self.reject_reopen.load(Ordering::SeqCst) {
            return Err(AdapterError::Rejected(
                "fixture retained conversation rejected".into(),
            ));
        }
        if self.fail_reopen_transiently.load(Ordering::SeqCst) {
            return Err(AdapterError::Unavailable(
                "fixture retained conversation temporarily unavailable".into(),
            ));
        }
        let cursors = self.reopen_output.cursors.lock().unwrap().clone();
        let (ack, mut acknowledgements) = mpsc::channel(1);
        for cursor in cursors {
            updates
                .send(ConversationUpdate::RuntimeOutput {
                    receipt: RuntimeOutputReceipt::new(
                        RuntimeOutputSource {
                            stream_id: "d99003d1-0c3c-46cd-aa47-9bbdbed50b12".into(),
                            cursor,
                            native_conversation_id: native.into(),
                            payload_sha256: format!("{cursor:064x}"),
                        },
                        ack.clone(),
                    ),
                    update: None,
                })
                .await
                .map_err(|_| AdapterError::Unavailable("fixture receiver closed".into()))?;
            self.reopen_output.submitted.lock().unwrap().push(cursor);
            // The hosted opener cannot become ready until custody is transferred.
            let source = acknowledgements
                .recv()
                .await
                .ok_or_else(|| AdapterError::Unavailable("fixture ACK channel closed".into()))?;
            self.reopen_output
                .acknowledged
                .lock()
                .unwrap()
                .push(source.cursor);
        }
        Ok(())
    }
}

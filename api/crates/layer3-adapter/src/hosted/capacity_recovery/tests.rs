use super::*;
use crate::agent_capacity_recovery::tests::{confirmation, request};
use crate::ConversationInputSender;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Native {
    calls: AtomicUsize,
    inputs: std::sync::Arc<AtomicUsize>,
    mismatch: bool,
    unavailable: bool,
}

#[async_trait]
impl CapacityRecoveryNativeTransport for Native {
    async fn reattach_existing(
        &self,
        request: &CapacityRecoveryRequest,
        _: mpsc::Sender<ConversationUpdate>,
    ) -> CapacityRecoveryResult<(ConversationChannel, CapacityRecoveryConfirmation)> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.unavailable {
            return Err(CapacityRecoveryError::Unavailable);
        }
        let inputs = self.inputs.clone();
        let input = ConversationInputSender::spawn(1, move |_| {
            let inputs = inputs.clone();
            async move {
                inputs.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        });
        Ok((
            ConversationChannel {
                input,
                conversation_id: Some(if self.mismatch {
                    "wrong-native".into()
                } else {
                    request.target.conversation_id.clone()
                }),
            },
            confirmation(request),
        ))
    }
}

#[tokio::test]
async fn restricted_transport_validates_without_sending_input_or_retrying() {
    let request = request(Some(5));
    for (mismatch, unavailable) in [(false, false), (true, false), (false, true)] {
        let transport = Native {
            calls: AtomicUsize::new(0),
            inputs: Default::default(),
            mismatch,
            unavailable,
        };
        let (updates, _receiver) = mpsc::channel(1);
        let result = reattach(Some(&transport), &request, updates).await;
        if unavailable {
            assert!(matches!(result, Err(CapacityRecoveryError::Unavailable)));
        } else if mismatch {
            assert!(matches!(
                result,
                Err(CapacityRecoveryError::IdentityConflict)
            ));
        } else {
            let connection = result.unwrap();
            assert_eq!(connection.confirmation(), &confirmation(&request));
            let (_channel, observed) = connection.into_parts();
            observed.validate_for(&request).unwrap();
        }
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
        assert_eq!(transport.inputs.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn absent_native_producer_is_typed_unsupported_without_ordinary_fallback() {
    let (updates, _receiver) = mpsc::channel(1);
    assert!(matches!(
        reattach(None, &request(None), updates).await,
        Err(CapacityRecoveryError::Unsupported)
    ));
}

#[tokio::test]
async fn invalid_request_is_refused_before_contacting_native_producer() {
    let transport = Native {
        calls: AtomicUsize::new(0),
        inputs: Default::default(),
        mismatch: false,
        unavailable: false,
    };
    let mut request = request(None);
    request.writer_fence = 0;
    let (updates, _receiver) = mpsc::channel(1);
    assert!(matches!(
        reattach(Some(&transport), &request, updates).await,
        Err(CapacityRecoveryError::IdentityConflict)
    ));
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
}

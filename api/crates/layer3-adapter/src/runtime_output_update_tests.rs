use super::*;

fn effect() -> ConversationUpdate {
    ConversationUpdate::Failed {
        message: "retained payload".repeat(4096),
        capability: None,
    }
}

#[test]
fn shared_payload_survives_clones_until_its_final_owner_drops() {
    let original = RuntimeOutputUpdate::new(effect(), true);
    let Storage::Shared(value) = &original.0 else {
        panic!("shared ownership must be selected explicitly")
    };
    let weak = Arc::downgrade(value);
    let copies: Vec<_> = (0..256).map(|_| original.clone()).collect();
    for copy in &copies {
        assert!(std::ptr::eq(&*original, &**copy));
        let ConversationUpdate::Failed { message, .. } = &**copy else {
            panic!("original domain effect must survive cloning")
        };
        assert_eq!(message.len(), "retained payload".len() * 4096);
    }
    drop(original);
    assert!(weak.upgrade().is_some());
    drop(copies);
    assert!(weak.upgrade().is_none());
}

#[test]
fn disabled_payload_preserves_legacy_independent_ownership_and_debug() {
    let original = RuntimeOutputUpdate::new(effect(), false);
    let copy = original.clone();
    assert!(!copy.is_shared());
    assert!(!std::ptr::eq(&*original, &*copy));
    let (
        ConversationUpdate::Failed { message: first, .. },
        ConversationUpdate::Failed {
            message: second, ..
        },
    ) = (&*original, &*copy)
    else {
        panic!("original effect must remain readable")
    };
    assert_ne!(first.as_ptr(), second.as_ptr());
    assert_eq!(first, second);
    assert_eq!(format!("{original:?}"), format!("{:?}", &*original));
}

#[test]
fn cloning_or_dropping_a_shared_envelope_does_not_acknowledge_it() {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let update = ConversationUpdate::RuntimeOutput {
        receipt: crate::RuntimeOutputReceipt::new(
            crate::RuntimeOutputSource {
                stream_id: "test-stream".into(),
                cursor: 1,
                native_conversation_id: "test-conversation".into(),
                payload_sha256: "a".repeat(64),
            },
            sender,
        ),
        update: Some(RuntimeOutputUpdate::new(effect(), true)),
    };
    let cloned = update.clone();
    drop(update);
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    drop(cloned);
    assert!(matches!(
        receiver.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));
}

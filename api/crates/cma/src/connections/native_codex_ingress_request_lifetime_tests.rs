use super::*;

#[test]
fn retained_request_clone_keeps_its_attachment_after_callers_drop() {
    let policy = attached_policy(1);
    let attachment = policy.admit_attachment().unwrap();
    let source = scope(attachment.policy());
    let result = NativeRefreshRequest::Initialized
        .encode_owned(&source, attachment.policy())
        .unwrap();
    let last = result.clone();
    drop((result, source, attachment));
    assert!(policy.admit_attachment().is_err());
    assert_eq!(last.expose(), "{\"method\":\"initialized\"}\n");
    drop(last);
    assert!(policy.admit_attachment().is_ok());
}

#[test]
fn partial_writer_unwind_returns_the_allowance() {
    let (root, policy) = policy();
    let source = scope(&policy);
    let baseline = root.available_bytes();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        RequestBuffer::admit(&source, &policy, 64)
            .unwrap()
            .finish(|writer| {
                writer.write_all(b"synthetic partial output").unwrap();
                panic!("synthetic writer unwind");
            })
    }));
    assert!(result.is_err());
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn invalid_utf8_test_writer_returns_static_failure() {
    let (root, policy) = policy();
    let source = scope(&policy);
    let baseline = root.available_bytes();
    let result = RequestBuffer::admit(&source, &policy, 4)
        .unwrap()
        .finish(|writer| {
            writer.write_all(&[0xff]).unwrap();
            Ok(())
        });
    assert_eq!(result.unwrap_err(), NativeFrameError::Unavailable);
    assert_eq!(root.available_bytes(), baseline);
}

use super::*;

#[tokio::test]
async fn fragmented_utf8_with_interleaved_ping_uses_the_upstream_collector() {
    let input = Input::new(&[0x01, 1, 0xe2, 0x89, 0, 0x80, 2, 0x82, 0xac], true);
    let admitted = Arc::new(Mutex::new(Vec::new()));
    let observed = admitted.clone();
    let admission = move |_: &FrameHeader, length| {
        observed.lock().unwrap().push(length);
        Ok(())
    };
    let mut socket = WebSocketStream::from_raw_socket(
        AdmissionIo::after_upgrade(input, admission),
        Role::Client,
        None,
    )
    .await;
    assert_eq!(socket.next().await.unwrap().unwrap(), Message::Ping(vec![]));
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Text("€".into())
    );
    assert_eq!(*admitted.lock().unwrap(), [1, 0, 2]);
}

#[tokio::test]
async fn masked_extended_header_is_admitted_after_all_four_mask_bytes() {
    let input = Input::new(
        &[0x82, 255, 0, 0, 0, 0, 0, 0, 0, 1, 1, 2, 3, 4, b'x' ^ 1],
        true,
    );
    let consumed = input.consumed.clone();
    let admission = move |header: &FrameHeader, length| {
        assert_eq!(consumed.load(Ordering::SeqCst), 14);
        assert_eq!(header.mask, Some([1, 2, 3, 4]));
        assert_eq!(length, 1);
        Ok(())
    };
    let mut socket = WebSocketStream::from_raw_socket(
        AdmissionIo::after_upgrade(input, admission),
        Role::Server,
        None,
    )
    .await;
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Binary(vec![b'x'])
    );
}

#[tokio::test]
async fn invalid_protocol_frames_still_fail_in_the_pinned_protocol_implementation() {
    let cases: &[(&'static [u8], Role)] = &[
        (&[0xc2, 1, b'x'], Role::Client),                   // reserved bit
        (&[0x83, 0], Role::Client),                         // reserved opcode
        (&[0x80, 1, b'x'], Role::Client),                   // no preceding fragment
        (&[0x81, 1, 0xff], Role::Client),                   // invalid UTF-8
        (&[0x09, 0], Role::Client),                         // fragmented control
        (&[0x82, 1, b'x'], Role::Server),                   // missing mask
        (&[0x82, 129, 1, 2, 3, 4, b'x' ^ 1], Role::Client), // forbidden mask
        (&[0x01, 1, 0xe2, 0x80, 0], Role::Client),          // incomplete UTF-8
    ];
    for &(bytes, role) in cases {
        let mut socket = WebSocketStream::from_raw_socket(
            AdmissionIo::after_upgrade(Input::new(bytes, true), |_: &FrameHeader, _| Ok(())),
            role,
            None,
        )
        .await;
        assert!(socket.next().await.unwrap().is_err(), "wire {bytes:?}");
        assert!(socket.next().await.is_none());
    }
}

#[tokio::test]
async fn full_width_length_reaches_admission_without_host_arithmetic_or_payload_reads() {
    let input = Input::new(
        &[0x82, 127, 255, 255, 255, 255, 255, 255, 255, 255, b'x'],
        false,
    );
    let consumed = input.consumed.clone();
    let mut io = AdmissionIo::after_upgrade(input, |_: &FrameHeader, length| {
        assert_eq!(length, u64::MAX);
        Err(io::ErrorKind::PermissionDenied.into())
    });
    let mut output = [0];
    assert_eq!(
        io.read(&mut output).await.unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(consumed.load(Ordering::SeqCst), 10);
}

#[tokio::test]
async fn header_can_be_delivered_in_small_codec_reads_without_repeated_admission() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = attempts.clone();
    let mut io = AdmissionIo::after_upgrade(
        Input::new(&[0x82, 126, 0, 1, b'x'], false),
        move |_: &FrameHeader, _| {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        },
    );
    let mut output = [0; 5];
    for byte in &mut output {
        assert_eq!(io.read(std::slice::from_mut(byte)).await.unwrap(), 1);
    }
    assert_eq!(output, [0x82, 126, 0, 1, b'x']);
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

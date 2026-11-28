//! Exercise the shipped Python relay with real pipes and TCP under backpressure.
use super::*;
use std::process::Stdio;
use tokio::{
    io::AsyncWriteExt,
    net::TcpStream,
    process::{Child, ChildStdin, ChildStdout, Command},
};

#[tokio::test]
async fn relay_readiness_distinguishes_an_early_close_from_protocol_rejection() {
    assert_eq!(
        read_ready(&mut &b""[..]).await,
        Err(RelayReadinessFailure::Unreadable)
    );
    assert_eq!(
        read_ready(&mut &b"BAD-EXEC-1\n"[..]).await,
        Err(RelayReadinessFailure::Invalid)
    );
    assert_eq!(read_ready(&mut &READY[..]).await, Ok(()));
}

async fn start() -> (Child, ChildStdin, ChildStdout, TcpStream) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    // Only the port changes so independent tests never contend for runtime port 9090.
    let code = RELAY.replace(
        "(\"127.0.0.1\", 9090)",
        &format!("(\"127.0.0.1\", {})", listener.local_addr().unwrap().port()),
    );
    let mut child = Command::new("python3")
        .args(["-I", "-u", "-c", &code])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let peer = tokio::time::timeout(Duration::from_secs(5), listener.accept())
        .await
        .unwrap()
        .unwrap()
        .0;
    let stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut marker = [0; READY.len()];
    tokio::time::timeout(Duration::from_secs(5), stdout.read_exact(&mut marker))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(marker, READY);
    (child, stdin, stdout, peer)
}

async fn finished(mut child: Child) {
    assert!(tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
    let mut errors = Vec::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut errors)
        .await
        .unwrap();
    assert!(errors.is_empty());
}

#[tokio::test]
async fn runtime_exec_relay_preserves_full_duplex_bytes_and_drains_on_peer_exit() {
    let (child, mut stdin, mut stdout, mut peer) = start().await;
    let expected: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
    let echoed = expected.clone();
    let server = tokio::spawn(async move {
        let mut count = 0;
        let mut buffer = [0; 8192];
        while count < echoed.len() {
            let n = peer.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            assert_eq!(&buffer[..n], &echoed[count..count + n]);
            peer.write_all(&buffer[..n]).await.unwrap();
            count += n;
        }
    });
    let mut actual = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        tokio::try_join!(stdin.write_all(&expected), stdout.read_to_end(&mut actual)).unwrap();
    })
    .await
    .unwrap();
    assert_eq!(actual, expected);
    server.await.unwrap();
    finished(child).await;
}

#[tokio::test]
async fn runtime_exec_relay_stdin_cancellation_exits_under_output_backpressure() {
    let (child, stdin, _stdout, mut peer) = start().await;
    // Fill the stdout pipe, relay's 64 KiB buffer and TCP window without reading stdout.
    let data = vec![0x61; 16 * 1024 * 1024];
    assert!(
        tokio::time::timeout(Duration::from_millis(200), peer.write_all(&data))
            .await
            .is_err()
    );
    drop(stdin);
    finished(child).await;
}

#[tokio::test]
async fn runtime_exec_relay_pipe_cancellation_exits_under_input_backpressure() {
    for close_stdin in [false, true] {
        let (child, mut stdin, stdout, _peer) = start().await;
        // The peer never reads: both pipes and the bounded relay buffer fill.
        let data = vec![0x62; 16 * 1024 * 1024];
        assert!(
            tokio::time::timeout(Duration::from_millis(200), stdin.write_all(&data))
                .await
                .is_err()
        );
        if close_stdin {
            drop(stdin);
        } else {
            drop(stdout);
        }
        finished(child).await;
    }
}

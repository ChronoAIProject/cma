//! Fixed non-writing stdio-to-loopback transport used to reach a runtime.
//! Its isolated interpreter and exact script bytes are also writer-fence evidence.

use sha2::{Digest as _, Sha256};

pub const PROGRAM: &str = "/usr/bin/python3";
pub const SCRIPT: &str = include_str!("runtime_control_relay.py");
// Retained previous relay script digest for rolling upgrades across replicas.
const RETAINED_SCRIPT_SHA256: [[u8; 32]; 1] = [[
    0xe5, 0x95, 0x83, 0x5b, 0x63, 0x14, 0xff, 0x07, 0x70, 0xfd, 0xf9, 0x78, 0x17, 0xd6, 0x92, 0x02,
    0x95, 0xea, 0x38, 0x4a, 0x5a, 0xc3, 0xbf, 0xdd, 0x04, 0x76, 0xb9, 0x0c, 0x81, 0xb7, 0x9a, 0x4f,
]];

pub fn command() -> [&'static str; 5] {
    [PROGRAM, "-I", "-u", "-c", SCRIPT]
}

pub fn matches_command_line(bytes: &[u8]) -> bool {
    let Some(body) = bytes.strip_suffix(&[0]) else {
        return false;
    };
    let mut parts = body.split(|byte| *byte == 0);
    let prefix_matches = [PROGRAM, "-I", "-u", "-c"]
        .into_iter()
        .all(|expected| parts.next() == Some(expected.as_bytes()));
    if !prefix_matches {
        return false;
    }
    let Some(script) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    if script == SCRIPT.as_bytes() {
        return true;
    }
    let digest: [u8; 32] = Sha256::digest(script).into();
    RETAINED_SCRIPT_SHA256.contains(&digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::process::Command;

    #[test]
    fn only_the_exact_isolated_nonwriting_relay_command_is_allowed() {
        let encode = |args: &[&str]| {
            args.iter()
                .flat_map(|arg| arg.as_bytes().iter().copied().chain(std::iter::once(0)))
                .collect::<Vec<_>>()
        };
        assert!(matches_command_line(&encode(&command())));
        for args in [
            vec![PROGRAM, "-u", "-c", SCRIPT],
            vec![PROGRAM, "-I", "-u", "-c", "print('other program')"],
            vec!["python3", "-I", "-u", "-c", SCRIPT],
            vec![PROGRAM, "-I", "-u", "-c", SCRIPT, "extra"],
        ] {
            assert!(!matches_command_line(&encode(&args)));
        }
        let mut unterminated = encode(&command());
        unterminated.pop();
        assert!(!matches_command_line(&unterminated));
    }

    fn unused_loopback_port() -> u16 {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    }

    #[tokio::test]
    async fn relay_retries_connection_refused_until_runtime_listener_binds() {
        let port = unused_loopback_port();
        let code = SCRIPT.replace("(\"127.0.0.1\", 9090)", &format!("(\"127.0.0.1\", {port})"));
        let mut child = Command::new("python3")
            .args(["-I", "-u", "-c", &code])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        // Let the relay hit initial ConnectionRefusedError before binding.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            child.try_wait().unwrap().is_none(),
            "relay exited before retry window elapsed"
        );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap();
        let (mut peer, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
            .await
            .expect("relay did not connect after listener bound")
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let mut marker = [0; 11];
        tokio::time::timeout(Duration::from_secs(3), stdout.read_exact(&mut marker))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&marker, b"CMA-EXEC-1\n");

        stdin.write_all(b"ping").await.unwrap();
        let mut received = [0; 4];
        peer.read_exact(&mut received).await.unwrap();
        assert_eq!(&received, b"ping");
        peer.write_all(b"pong").await.unwrap();
        drop(peer);
        let mut reply = Vec::new();
        stdout.read_to_end(&mut reply).await.unwrap();
        assert_eq!(reply, b"pong");
        drop(stdin);

        let status = tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success());
        let mut stderr = Vec::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_end(&mut stderr)
            .await
            .unwrap();
        assert!(stderr.is_empty());
    }

    #[tokio::test]
    async fn relay_exits_70_when_connection_refused_exceeds_deadline() {
        let port = unused_loopback_port();
        let code = SCRIPT
            .replace("(\"127.0.0.1\", 9090)", &format!("(\"127.0.0.1\", {port})"))
            .replace(
                "CONNECT_RETRY_SECONDS = 2.5",
                "CONNECT_RETRY_SECONDS = 0.25",
            );
        let mut child = Command::new("python3")
            .args(["-I", "-u", "-c", &code])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let _stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let status = tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .expect("relay did not exit after retry deadline")
            .unwrap();
        assert_eq!(status.code(), Some(70));
        let mut out = Vec::new();
        let mut err = Vec::new();
        stdout.read_to_end(&mut out).await.unwrap();
        stderr.read_to_end(&mut err).await.unwrap();
        assert!(out.is_empty());
        assert!(err.is_empty());
    }

    #[tokio::test]
    async fn relay_stops_retrying_promptly_when_caller_closes_stdin() {
        let port = unused_loopback_port();
        let code = SCRIPT.replace("(\"127.0.0.1\", 9090)", &format!("(\"127.0.0.1\", {port})"));
        let mut child = Command::new("python3")
            .args(["-I", "-u", "-c", &code])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let started = Instant::now();
        tokio::time::sleep(Duration::from_millis(80)).await;
        drop(stdin);
        let status = tokio::time::timeout(Duration::from_secs(1), child.wait())
            .await
            .expect("relay hung during retry after caller closed stdin")
            .unwrap();
        assert_eq!(status.code(), Some(70));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}

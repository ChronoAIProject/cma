//! Exercise the real startup entry point in an isolated process.
#[cfg(unix)]
#[path = "mongo.rs"]
mod mongo;
#[cfg(unix)]
#[path = "process.rs"]
mod process;
#[cfg(unix)]
#[tokio::test]
async fn actual_startup_sigterm_cancels_delayed_composition_before_membership() {
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let key = SyntheticKey::new();
    let mut child = process::command(
        &format!("mongodb://{address}/?directConnection=true&serverSelectionTimeoutMS=60000"),
        "cma_startup_synthetic",
        &key.0,
    )
    .spawn()
    .unwrap();
    // A Mongo handshake proves serve() reached asynchronous composition with
    // its real signal handlers installed. The dependency never answers, so no
    // membership write or worker activation can be reached.
    let handshake = tokio::time::timeout(Duration::from_secs(15), async {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut byte = [0];
        socket.read_exact(&mut byte).await.unwrap();
        socket
    })
    .await;
    if handshake.is_err() {
        child.kill().await.unwrap();
        let output = child.wait_with_output().await.unwrap();
        panic!(
            "startup did not reach Mongo handshake: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let _socket = handshake.unwrap();
    assert!(std::process::Command::new("kill")
        .args(["-TERM", &child.id().unwrap().to_string()])
        .status()
        .unwrap()
        .success());
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("ACTUAL_STARTUP_CANCELLED"));
}

#[cfg(unix)]
struct SyntheticKey(std::path::PathBuf);

#[cfg(unix)]
impl SyntheticKey {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("cma-startup-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path, uuid::Uuid::new_v4().simple().to_string()).unwrap();
        Self(path)
    }
}

#[cfg(unix)]
impl Drop for SyntheticKey {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).expect("remove synthetic startup credential");
    }
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "subprocess fixture for the actual startup entry point"]
async fn actual_startup_child() {
    assert_eq!(std::env::var("CMA_TEST_STARTUP_CHILD").as_deref(), Ok("1"));
    let result = super::serve().await;
    match std::env::var("CMA_TEST_STARTUP_OUTCOME").as_deref() {
        Ok("serving") => assert_eq!(result, Ok(())),
        Ok(expected) => assert!(
            matches!(&result, Err(error) if error.starts_with(expected)),
            "{result:?}"
        ),
        Err(_) => assert_eq!(result, Err("termination requested during startup".into())),
    }
    println!("ACTUAL_STARTUP_CANCELLED");
}

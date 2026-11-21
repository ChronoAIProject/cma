use std::{path::PathBuf, process::Stdio, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    time::timeout,
};

const SWITCH: &str = "--resource-execution-bootstrap-v1";
const PREPARED: &[u8] = b"CMA_RESOURCE_PREPARED_V1\n";
const ENTER: &[u8] = b"CMA_RESOURCE_ENTER_V1\n";
const DEADLINE: Duration = Duration::from_secs(5);

struct NativeFixture {
    root: PathBuf,
    program: PathBuf,
    marker: PathBuf,
}

impl NativeFixture {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "cma-resource-bootstrap-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&root).unwrap();
        let program = root.join("native");
        let marker = root.join("started");
        std::fs::write(&program, b"#!/bin/sh\nprintf started > \"$1\"\n/bin/cat\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            root,
            program,
            marker,
        }
    }

    fn spawn(&self) -> Child {
        command()
            .arg("--")
            .arg(&self.program)
            .arg(&self.marker)
            .spawn()
            .unwrap()
    }
}

impl Drop for NativeFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_layer4-runtime"));
    command
        .arg(SWITCH)
        .env_clear()
        // A bootstrap must not parse supervisor configuration or require it.
        .env("CHRONO_RESOURCE_EXECUTION_FENCING_ENABLED", "invalid")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

async fn prepared(child: &mut Child) {
    let mut observed = vec![0; PREPARED.len()];
    timeout(
        DEADLINE,
        child.stdout.as_mut().unwrap().read_exact(&mut observed),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(observed, PREPARED);
}

async fn finish(child: Child) -> std::process::Output {
    timeout(DEADLINE, child.wait_with_output())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn eof_partial_and_wrong_release_never_load_the_native_program() {
    for release in [
        b"".as_slice(),
        &ENTER[..ENTER.len() - 1],
        b"CMA_RESOURCE_OTHER_V1\n",
    ] {
        let fixture = NativeFixture::new();
        let mut child = fixture.spawn();
        prepared(&mut child).await;
        assert!(!fixture.marker.exists());
        let mut input = child.stdin.take().unwrap();
        input.write_all(release).await.unwrap();
        drop(input);
        let result = finish(child).await;
        assert_eq!(result.status.code(), Some(125));
        assert!(!fixture.marker.exists());
        assert!(result.stdout.is_empty());
        assert_eq!(result.stderr, b"resource execution bootstrap refused\n");
    }
}

#[tokio::test]
async fn explicit_release_preserves_all_queued_native_input() {
    let fixture = NativeFixture::new();
    let mut child = fixture.spawn();
    prepared(&mut child).await;
    assert!(!fixture.marker.exists());
    let payload = b"synthetic native input after the entry token\n".repeat(4096);
    let mut bytes = ENTER.to_vec();
    bytes.extend_from_slice(&payload);
    let mut input = child.stdin.take().unwrap();
    let write = async move {
        input.write_all(&bytes).await.unwrap();
        drop(input);
    };
    let ((), result) = tokio::join!(write, finish(child));
    assert!(result.status.success());
    assert_eq!(std::fs::read(&fixture.marker).unwrap(), b"started");
    assert_eq!(result.stdout, payload);
    assert!(result.stderr.is_empty());
}

#[tokio::test]
async fn native_program_is_resolved_only_after_explicit_release() {
    let fixture = NativeFixture::new();
    let mut child = fixture.spawn();
    prepared(&mut child).await;
    std::fs::remove_file(&fixture.program).unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(ENTER)
        .await
        .unwrap();
    let result = finish(child).await;
    assert_eq!(result.status.code(), Some(126));
    assert!(!fixture.marker.exists());
}

#[tokio::test]
async fn native_process_inherits_no_supervisor_environment() {
    let mut child = command()
        .args([
            "--",
            "/bin/sh",
            "-c",
            "printf '%s' \"${CHRONO_RESOURCE_EXECUTION_FENCING_ENABLED-unset}\"; pwd",
        ])
        .spawn()
        .unwrap();
    prepared(&mut child).await;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(ENTER)
        .await
        .unwrap();
    let result = finish(child).await;
    assert!(result.status.success());
    assert_eq!(result.stdout, b"unset/\n");
}

#[tokio::test]
async fn malformed_or_oversized_launch_is_refused_before_readiness() {
    for arguments in [
        vec![],
        vec!["--".to_owned()],
        vec!["--".to_owned(), "relative-program".to_owned()],
        vec![
            "--".to_owned(),
            "/bin/true".to_owned(),
            "x".repeat(32 * 1024),
        ],
        std::iter::once("--".to_owned())
            .chain(std::iter::repeat_n("/bin/true".to_owned(), 257))
            .collect(),
    ] {
        let child = command().args(arguments).spawn().unwrap();
        let result = finish(child).await;
        assert_eq!(result.status.code(), Some(125));
        assert!(result.stdout.is_empty());
    }
}

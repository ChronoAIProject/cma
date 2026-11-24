use super::*;
use layer2_core::qualification::{ControlCheckpoint, ControlSchedule, ScheduleEffect};
use serde_json::{json, Value};

const BUILD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "cmaq-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ));
        let mut builder = fs::DirBuilder::new();
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700).create(&root).unwrap();
        Self(root)
    }

    fn socket(&self) -> PathBuf {
        self.0.join("c.sock")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn plan() -> Value {
    json!({
        "version": 1, "build": BUILD, "request_digest": vec![1; 32],
        "agent_digest": vec![2; 32], "lifetime_ms": 5000
    })
}

#[test]
fn plans_reject_unknown_fields_duplicate_fields_wrong_build_and_public_permissions() {
    let fixture = Fixture::new();
    let path = fixture.0.join("plan.json");
    let bytes = serde_json::to_vec(&plan()).unwrap();
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    use std::io::Write;
    file.write_all(&bytes).unwrap();
    assert!(Plan::open(&path, BUILD).is_ok());
    assert!(Plan::open(&path, "different-build").is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(Plan::open(&path, BUILD).is_err());
    let mut unknown = plan();
    unknown["actor"] = json!("unselected");
    assert!(Plan::parse(&serde_json::to_vec(&unknown).unwrap(), BUILD).is_err());
    let mut ambiguous = bytes;
    ambiguous.splice(1..1, b"\"version\":1,".iter().copied());
    assert!(Plan::parse(&ambiguous, BUILD).is_err());
}

async fn request(peer: &mut UnixStream, digest: [u8; 32], sequence: u8, action: &str) -> Value {
    let bytes = serde_json::to_vec(&json!({
        "plan_digest": digest, "sequence": sequence, "action": action
    }))
    .unwrap();
    peer.write_u32(bytes.len() as u32).await.unwrap();
    peer.write_all(&bytes).await.unwrap();
    let length = peer.read_u32().await.unwrap() as usize;
    let mut reply = vec![0; length];
    peer.read_exact(&mut reply).await.unwrap();
    serde_json::from_slice(&reply).unwrap()
}

#[tokio::test]
async fn local_socket_replays_exact_commands_and_disconnect_releases_the_handler() {
    let fixture = Fixture::new();
    let plan = Plan::parse(&serde_json::to_vec(&plan()).unwrap(), BUILD).unwrap();
    let digest = plan.digest;
    let context = plan.context;
    let registry = Registry::default();
    let server = ControlServer::bind(fixture.socket(), plan, registry.clone(), None, None).unwrap();
    assert_eq!(
        fs::symlink_metadata(fixture.socket()).unwrap().mode() & 0o777,
        0o600
    );
    let mut peer = UnixStream::connect(fixture.socket()).await.unwrap();
    assert_eq!(
        request(&mut peer, digest, 0, "arm").await["status"],
        "armed"
    );
    let handler = tokio::spawn(async move {
        registry
            .after_schedule(
                context,
                ControlSchedule {
                    agent_revision: 4,
                    effect: ScheduleEffect::Scheduled,
                },
            )
            .await;
    });
    let reached = request(&mut peer, digest, 1, "wait").await;
    assert_eq!(
        reached,
        json!({"status":"reached","agent_revision":4,"scheduled":true})
    );
    assert_eq!(request(&mut peer, digest, 1, "wait").await, reached);
    assert_eq!(
        request(&mut peer, digest, 0, "arm").await["status"],
        "armed"
    );
    assert!(!handler.is_finished());
    drop(peer);
    timeout(IO_TIMEOUT, handler).await.unwrap().unwrap();
    let mut reconnected = UnixStream::connect(fixture.socket()).await.unwrap();
    assert_eq!(
        request(&mut reconnected, digest, 0, "arm").await["status"],
        "refused"
    );
    drop(server);
    assert!(!fixture.socket().exists());
}

#[tokio::test]
async fn command_frames_refuse_oversized_and_ambiguous_input() {
    let digest = serde_json::to_string(&[1_u8; 32].as_slice()).unwrap();
    let ambiguous =
        format!(r#"{{"plan_digest":{digest},"sequence":0,"action":"arm","action":"release"}}"#);
    let (mut sender, mut reader) = UnixStream::pair().unwrap();
    sender.write_u32((MAX_FRAME + 1) as u32).await.unwrap();
    assert_eq!(
        read_request(&mut reader).await.unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    sender.write_u32(ambiguous.len() as u32).await.unwrap();
    sender.write_all(ambiguous.as_bytes()).await.unwrap();
    assert_eq!(
        read_request(&mut reader).await.unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}

#[tokio::test]
async fn changed_replay_closes_the_connection_and_releases_the_checkpoint() {
    let fixture = Fixture::new();
    let plan = Plan::parse(&serde_json::to_vec(&plan()).unwrap(), BUILD).unwrap();
    let digest = plan.digest;
    let context = plan.context;
    let registry = Registry::default();
    let _server =
        ControlServer::bind(fixture.socket(), plan, registry.clone(), None, None).unwrap();
    let mut peer = UnixStream::connect(fixture.socket()).await.unwrap();
    assert_eq!(
        request(&mut peer, digest, 0, "arm").await["status"],
        "armed"
    );
    let handler = tokio::spawn(async move {
        registry
            .after_schedule(
                context,
                ControlSchedule {
                    agent_revision: 4,
                    effect: ScheduleEffect::Scheduled,
                },
            )
            .await;
    });
    assert_eq!(
        request(&mut peer, digest, 1, "wait").await["status"],
        "reached"
    );
    let changed = serde_json::to_vec(&json!({
        "plan_digest": digest, "sequence": 1, "action": "release"
    }))
    .unwrap();
    peer.write_u32(changed.len() as u32).await.unwrap();
    peer.write_all(&changed).await.unwrap();
    assert_eq!(
        timeout(IO_TIMEOUT, peer.read_u8())
            .await
            .unwrap()
            .unwrap_err()
            .kind(),
        io::ErrorKind::UnexpectedEof
    );
    timeout(IO_TIMEOUT, handler).await.unwrap().unwrap();
}

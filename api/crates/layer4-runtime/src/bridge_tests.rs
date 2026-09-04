use layer3_adapter::AgentSlot;

use super::*;

fn launch() -> AgentLaunch {
    AgentLaunch {
        agent_id: AgentSlot::new("test-agent"),
        cwd: "/tmp".into(),
        home: "/tmp".into(),
        program: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            "while IFS= read -r line; do printf '%s\\n' \"$line\"; done".into(),
        ],
        env: BTreeMap::new(),
    }
}

#[tokio::test]
async fn separate_bridges_have_independent_streams() {
    let first = Bridge::start(&launch(), &BTreeMap::new()).unwrap();
    let second = Bridge::start(&launch(), &BTreeMap::new()).unwrap();
    let mut first_output = first.subscribe();
    let mut second_output = second.subscribe();
    assert!(first.send(b"one".to_vec()).await);
    assert!(second.send(b"two".to_vec()).await);
    assert_eq!(
        first_output.recv().await.unwrap(),
        Outbound::Frame(b"one".to_vec())
    );
    assert_eq!(
        second_output.recv().await.unwrap(),
        Outbound::Frame(b"two".to_vec())
    );
    first.terminate().await.unwrap();
    second.terminate().await.unwrap();
}

#[tokio::test]
async fn detached_output_is_replayed() {
    let bridge = Bridge::start(&launch(), &BTreeMap::new()).unwrap();
    assert!(bridge.send(b"held".to_vec()).await);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let backlog = bridge.backlog().await;
        if !backlog.is_empty() {
            assert_eq!(backlog, vec![Outbound::Frame(b"held".to_vec())]);
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    bridge.terminate().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn close_terminates_descendants_after_the_primary_has_exited() {
    let mut launch = launch();
    launch.args = vec!["-c".into(), "sleep 30 & printf '%s\\n' $!".into()];
    let bridge = Bridge::start(&launch, &BTreeMap::new()).unwrap();
    let mut output = bridge.subscribe();
    let descendant = match tokio::time::timeout(Duration::from_secs(2), output.recv())
        .await
        .unwrap()
        .unwrap()
    {
        Outbound::Frame(bytes) => String::from_utf8(bytes).unwrap().parse::<i32>().unwrap(),
        other => panic!("expected descendant PID, got {other:?}"),
    };
    tokio::time::timeout(Duration::from_secs(2), bridge.ended())
        .await
        .expect("bridge primary exited");
    assert_eq!(unsafe { libc::kill(descendant, 0) }, 0);
    bridge.terminate().await.unwrap();
    assert_ne!(unsafe { libc::kill(descendant, 0) }, 0);
}

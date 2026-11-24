use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::sync::Notify;

#[tokio::test]
async fn aggregate_caps_combined_bytes_and_recounts_rows_without_losing_reporters() {
    let members = Arc::new(Members::default());
    let mut services = Vec::new();
    let mut servers = Vec::new();
    let mut registered = Vec::new();
    for index in 0..MAX_REPORTERS {
        let id = format!("member-{index}");
        let rows: Vec<_> = (0..MAX_HOLDS_PER_REPORTER)
            .map(|row| {
                let mut value = hold(&format!("{index:02}-{row:02}-{}", "s".repeat(250)));
                value.owner = SubjectId("o".repeat(512));
                value
            })
            .collect();
        members.owners.lock().unwrap().extend(
            rows.iter()
                .map(|row| (row.sandbox_id.clone(), row.owner.clone())),
        );
        let source = service(&id, members.clone(), rows);
        let listener = server(source.router()).await;
        registered.push(member(&id, &listener.address));
        services.push(source);
        servers.push(listener);
    }
    *members.values.lock().unwrap() = registered;
    let view = services[0]
        .observe(&SubjectId("operator".into()))
        .await
        .unwrap();
    let bytes = serde_json::to_vec(&view).unwrap();
    assert!(bytes.len() <= MAX_BODY);
    assert!(view.truncated);
    assert_eq!(view.registered_reporters, MAX_REPORTERS);
    assert_eq!(view.unavailable_reporters, 0);
    assert_eq!(view.reports.len(), MAX_REPORTERS);
    assert!(view.observed_sandboxes > 0);
    assert!(view.observed_sandboxes < MAX_REPORTERS * MAX_HOLDS_PER_REPORTER);
    assert_eq!(
        view.observed_sandboxes,
        view.reports
            .iter()
            .map(|report| report.sample.holds.len())
            .sum::<usize>()
    );
    assert!(view
        .reports
        .iter()
        .any(|report| report.sample.holds.is_empty() && report.sample.truncated));
}

#[tokio::test]
async fn aggregate_deadline_returns_partial_observation_and_releases_slots() {
    let members = Arc::new(Members::default());
    let source = service("a", members.clone(), vec![]);
    let upstream = server(Router::new().route(
        PATH,
        get(|| async { std::future::pending::<StatusCode>().await }),
    ))
    .await;
    *members.values.lock().unwrap() = vec![
        member("a", "http://a.example.test/"),
        member("b", &upstream.address),
    ];
    let view = tokio::time::timeout(
        Duration::from_secs(6),
        source.observe(&SubjectId("operator".into())),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(view.unavailable_reporters, 1);
    assert_eq!(view.reports.len(), 1);
    assert_eq!(source.slots.available_permits(), 2);
    assert_eq!(source.peer_slots.available_permits(), 4);
}

#[tokio::test]
async fn concurrent_reads_share_peer_cap_and_cancellation_releases_all_slots() {
    let members = Arc::new(Members::default());
    let source = service("a", members.clone(), vec![]);
    let started = Arc::new(AtomicUsize::new(0));
    let arrived = Arc::new(Notify::new());
    let upstream = server(Router::new().route(
        PATH,
        get({
            let started = started.clone();
            let arrived = arrived.clone();
            move || {
                let started = started.clone();
                let arrived = arrived.clone();
                async move {
                    started.fetch_add(1, Ordering::SeqCst);
                    arrived.notify_one();
                    std::future::pending::<StatusCode>().await
                }
            }
        }),
    ))
    .await;
    let mut registered = vec![member("a", "http://a.example.test/")];
    registered.extend((0..8).map(|index| member(&format!("peer-{index}"), &upstream.address)));
    *members.values.lock().unwrap() = registered;
    let first = tokio::spawn({
        let source = source.clone();
        async move { source.observe(&SubjectId("operator-a".into())).await }
    });
    let second = tokio::spawn({
        let source = source.clone();
        async move { source.observe(&SubjectId("operator-b".into())).await }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while started.load(Ordering::SeqCst) < 4 {
            arrived.notified().await;
        }
        while source.slots.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(started.load(Ordering::SeqCst), 4);
    assert_eq!(source.peer_slots.available_permits(), 0);
    assert!(source
        .observe(&SubjectId("operator-c".into()))
        .await
        .is_err());
    first.abort();
    second.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    assert!(second.await.unwrap_err().is_cancelled());
    assert_eq!(source.slots.available_permits(), 2);
    assert_eq!(source.peer_slots.available_permits(), 4);
}

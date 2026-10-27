use super::super::source_worker::Attempt;
use super::*;
use std::sync::Mutex;

#[derive(Default)]
struct Port {
    heads: Mutex<Vec<RecordHead>>,
    refused: Mutex<BTreeMap<String, GatewayError>>,
    scans: Mutex<Vec<Option<String>>>,
    unavailable: Mutex<bool>,
}
impl SourcePort for Port {
    fn revision<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<String, GatewayError>> {
        Box::pin(async { Ok("revision".into()) })
    }
    fn connect<'a>(&'a self, _: &'a str, _: watch::Receiver<bool>) -> BoxFuture<'a, Attempt> {
        panic!("scanner must not connect")
    }
}
impl DiscoveryPort for Port {
    fn page<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Vec<RecordHead>, GatewayError>> {
        Box::pin(async move {
            self.scans.lock().unwrap().push(after.map(str::to_owned));
            if *self.unavailable.lock().unwrap() {
                return Err(GatewayError::Unavailable);
            }
            Ok(self
                .heads
                .lock()
                .unwrap()
                .iter()
                .filter(|head| after.is_none_or(|after| head.record_id.as_str() > after))
                .take(PAGE_SIZE)
                .cloned()
                .collect())
        })
    }
    fn eligible<'a>(
        &'a self,
        record: &'a str,
        owner: Option<&'a str>,
    ) -> BoxFuture<'a, Result<(), GatewayError>> {
        Box::pin(async move {
            if owner.is_some_and(|owner| owner != "alice") {
                return Err(GatewayError::Authority);
            }
            match self.refused.lock().unwrap().get(record) {
                Some(error) => Err(*error),
                None => Ok(()),
            }
        })
    }
}
fn id(index: usize) -> String {
    format!("00000000-0000-4000-8000-{index:012x}")
}
fn port(count: usize) -> Port {
    Port {
        heads: Mutex::new(
            (1..=count)
                .map(|index| RecordHead {
                    record_id: id(index),
                    owner: "alice".into(),
                    version: 1,
                })
                .collect(),
        ),
        ..Default::default()
    }
}
fn selection() -> Selection {
    Selection::new(&crate::NativeDiscordConfig {
        enabled: true,
        discovery_enabled: true,
        records: BTreeSet::new(),
    })
}
#[tokio::test]
async fn discovery_is_paged_bounded_and_preserves_canary_priority() {
    let port = port(90);
    let selected = selection();
    let pinned = BTreeSet::from([id(999)]);
    selected.insert(id(999));
    let mut scanner = Scanner::default();
    scanner.step(&port, &selected, &pinned).await.unwrap();
    assert_eq!(selected.len(), 33);
    scanner.step(&port, &selected, &pinned).await.unwrap();
    scanner.step(&port, &selected, &pinned).await.unwrap();
    assert_eq!(selected.len(), MAX_RECORDS);
    assert!(selected.contains(&id(999)));
    assert!(!selected.contains(&id(64)));
    assert_eq!(
        *port.scans.lock().unwrap(),
        [None, Some(id(32)), Some(id(64))]
    );
    assert!(scanner.cursor.is_none());
}
#[tokio::test]
async fn partial_failed_and_empty_scans_do_not_establish_deletion() {
    let port = port(70);
    let selected = selection();
    let mut scanner = Scanner::default();
    scanner
        .step(&port, &selected, &BTreeSet::new())
        .await
        .unwrap();
    *port.unavailable.lock().unwrap() = true;
    assert!(scanner
        .step(&port, &selected, &BTreeSet::new())
        .await
        .is_err());
    assert_eq!(selected.len(), 32);
    assert_eq!(scanner.cursor, Some(id(32)));
    *port.unavailable.lock().unwrap() = false;
    port.heads.lock().unwrap().clear();
    scanner
        .step(&port, &selected, &BTreeSet::new())
        .await
        .unwrap();
    assert_eq!(selected.len(), 32);
}
#[tokio::test]
async fn exact_revocation_releases_capacity_but_transient_failure_does_not() {
    let port = port(65);
    let selected = selection();
    for index in 1..=64 {
        selected.insert(id(index));
    }
    port.refused
        .lock()
        .unwrap()
        .insert(id(1), GatewayError::Authority);
    port.refused
        .lock()
        .unwrap()
        .insert(id(2), GatewayError::Unavailable);
    let mut scanner = Scanner::default();
    for _ in 0..3 {
        scanner
            .step(&port, &selected, &BTreeSet::new())
            .await
            .unwrap();
    }
    assert!(!selected.contains(&id(1)));
    assert!(selected.contains(&id(2)));
    assert!(selected.contains(&id(65)));
    assert_eq!(selected.len(), 64);
}
#[tokio::test]
async fn owner_mismatch_non_native_and_legacy_index_rows_cannot_activate() {
    let port = port(3);
    port.heads.lock().unwrap()[0].owner = "bob".into();
    port.heads.lock().unwrap().push(RecordHead {
        record_id: "legacy-row".into(),
        owner: "alice".into(),
        version: 1,
    });
    port.refused
        .lock()
        .unwrap()
        .insert(id(2), GatewayError::InvalidConfiguration);
    let selected = selection();
    Scanner::default()
        .step(&port, &selected, &BTreeSet::new())
        .await
        .unwrap();
    assert_eq!(selected.records(), BTreeSet::from([id(3)]));
    let restarted = selection();
    Scanner::default()
        .step(&port, &restarted, &BTreeSet::new())
        .await
        .unwrap();
    assert_eq!(selected.records(), restarted.records());
}

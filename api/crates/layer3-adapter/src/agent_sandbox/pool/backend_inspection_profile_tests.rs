//! Serial scheduling is the control; both versions run real native inspection.
use super::*;
use std::time::Instant;

async fn sample(candidate: bool, parent_ms: u64, read_ms: u64) -> serde_json::Value {
    let fixture = Fixture::with_read_delay(Duration::from_millis(read_ms)).await;
    let (release, admission) = InspectionAdmission::pending();
    let started = Instant::now();
    let result = if candidate {
        let inspect = fixture.backend.inspect_native_runtime_after(
            &fixture.original.bound.runtime,
            &fixture.original.bound.request,
            &fixture.original.home,
            &admission,
        );
        let parents = async {
            if parent_ms > 0 {
                tokio::time::sleep(Duration::from_millis(parent_ms)).await;
            }
            release.allow();
        };
        tokio::join!(parents, inspect).1
    } else {
        if parent_ms > 0 {
            tokio::time::sleep(Duration::from_millis(parent_ms)).await;
        }
        fixture
            .backend
            .inspect_native_runtime(
                &fixture.original.bound.runtime,
                &fixture.original.bound.request,
                &fixture.original.home,
            )
            .await
    };
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let reads = fixture.effects.calls.lock().unwrap().len();
    let binds = fixture.transport.binds.load(Ordering::SeqCst);
    let statuses = fixture.original.transport.calls.load(Ordering::SeqCst);
    let success = result.as_ref().is_ok_and(|v| {
        v.as_ref().is_some_and(|i| {
            i.observation.handle == fixture.original.bound.runtime
                && i.status.as_ref().is_some_and(|s| s.credential_ready)
        })
    });
    let row = json!({"candidate":candidate,"parent_ms":parent_ms,"read_ms":read_ms,
        "elapsed_ms":elapsed_ms,"reads":reads,"binds":binds,"statuses":statuses,"success":success});
    eprintln!("INSPECTION_PROFILE {row}");
    assert!(success);
    assert_eq!(binds, 1);
    assert_eq!(statuses, 1);
    assert!(reads == 12 || candidate && reads == 18);
    fixture.effects.assert_read_only();
    row
}

#[tokio::test]
#[ignore = "local alternating component timing; run explicitly with nocapture"]
async fn local_serial_and_overlapped_native_inspection_profile() {
    for (parent_ms, read_ms) in [(0, 0), (40, 80), (80, 20)] {
        for pair in 0..20 {
            for candidate in if pair % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                sample(candidate, parent_ms, read_ms).await;
            }
        }
    }
}

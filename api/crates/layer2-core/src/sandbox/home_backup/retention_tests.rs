use super::retention::*;
use super::{HomeBackupError, HomeBackupRetentionClass as Class};

fn point(id: &str, ready_at: &str) -> ReadyPoint {
    ReadyPoint {
        backup_id: id.into(),
        ready_at: ready_at.into(),
    }
}

#[test]
fn first_hourly_and_sgt_daily_points_receive_fixed_deadlines() {
    let points = [
        point("a", "2026-11-16T15:00:01Z"),
        point("b", "2026-11-16T15:30:00Z"),
        point("c", "2026-11-16T16:00:00Z"),
        point("d", "2026-11-16T17:05:00Z"),
    ];
    let result = retain_ready_points(&points).unwrap();
    assert_eq!(result[0].retention.class, Class::Daily);
    assert_eq!(result[0].retention.expires_at, "2026-12-16T15:00:01Z");
    assert_eq!(result[1].retention.class, Class::Short);
    assert_eq!(result[1].retention.expires_at, "2026-11-16T18:00:00Z");
    // 16:00 UTC is midnight in Singapore, while 17:05 is the next hour.
    assert_eq!(result[2].retention.class, Class::Daily);
    assert_eq!(result[2].retention.expires_at, "2026-12-16T16:00:00Z");
    assert_eq!(result[3].retention.class, Class::Hourly);
    assert_eq!(result[3].retention.expires_at, "2026-11-23T17:05:00Z");
}

#[test]
fn short_point_waits_for_a_strictly_newer_ready_point_with_a_hard_cap() {
    let mut points = vec![
        point("a", "2026-11-16T00:00:00Z"),
        point("b", "2026-11-16T00:30:00Z"),
        point("c", "2026-11-16T00:30:00Z"),
    ];
    let waiting = retain_ready_points(&points).unwrap();
    assert_eq!(waiting[1].retention.expires_at, "2026-12-16T00:30:00Z");
    assert_eq!(waiting[2].retention.expires_at, "2026-12-16T00:30:00Z");
    points.push(point("d", "2026-11-16T00:45:00Z"));
    let superseded = retain_ready_points(&points).unwrap();
    assert_eq!(superseded[1].retention.expires_at, "2026-11-16T02:45:00Z");
    assert_eq!(superseded[2].retention.expires_at, "2026-11-16T02:45:00Z");
    points[3].ready_at = "2026-12-16T00:29:59Z".into();
    let capped = retain_ready_points(&points).unwrap();
    assert_eq!(capped[1].retention.expires_at, "2026-12-16T00:30:00Z");
    assert_eq!(capped[2].retention.expires_at, "2026-12-16T00:30:00Z");
}

#[test]
fn classification_is_independent_of_processing_order_and_breaks_ties_by_identity() {
    let mut points = vec![
        point("b", "2026-11-16T00:00:00Z"),
        point("a", "2026-11-16T00:00:00Z"),
        point("c", "2026-11-16T01:00:00Z"),
    ];
    let result = retain_ready_points(&points).unwrap();
    assert_eq!(result[0].backup_id, "a");
    assert_eq!(result[0].retention.class, Class::Daily);
    assert_eq!(result[1].retention.class, Class::Short);
    assert_eq!(result[2].retention.class, Class::Hourly);
    points.reverse();
    assert_eq!(retain_ready_points(&points).unwrap(), result);
    assert!(retain_ready_points(&[]).unwrap().is_empty());
}

#[test]
fn malformed_timestamps_and_duplicate_or_empty_identities_are_refused() {
    for timestamp in ["2026-11-16", "2026-11-16T00:00:00+08:00", "invalid"] {
        assert!(matches!(
            retain_ready_points(&[point("a", timestamp)]),
            Err(HomeBackupError::Invalid(_))
        ));
    }
    for id in ["", " a", "a\n"] {
        assert!(retain_ready_points(&[point(id, "2026-11-16T00:00:00Z")]).is_err());
    }
    assert!(retain_ready_points(&[
        point("a", "2026-11-16T00:00:00Z"),
        point("a", "2026-11-16T01:00:00Z"),
    ])
    .is_err());
}

#[test]
fn controlled_clock_spans_sgt_midnight_and_deadlines_never_exceed_thirty_days() {
    let start = crate::seconds_from_rfc3339("2026-11-16T15:30:00Z").unwrap();
    let points: Vec<_> = (0..192)
        .map(|tick| ReadyPoint {
            backup_id: format!("point-{tick:03}"),
            ready_at: crate::rfc3339_at(
                std::time::UNIX_EPOCH + std::time::Duration::from_secs(start + tick * 1800),
            ),
        })
        .collect();
    let result = retain_ready_points(&points).unwrap();
    assert_eq!(result[0].retention.class, Class::Daily);
    assert_eq!(result[1].retention.class, Class::Daily);
    assert_eq!(result[2].retention.class, Class::Short);
    assert_eq!(result[49].retention.class, Class::Daily);
    for decision in result {
        let ready = points
            .iter()
            .find(|p| p.backup_id == decision.backup_id)
            .unwrap();
        let at = crate::seconds_from_rfc3339(&ready.ready_at).unwrap();
        let expires = crate::seconds_from_rfc3339(&decision.retention.expires_at).unwrap();
        assert!(expires <= at + 30 * 86400);
        assert!(expires >= at);
    }
}

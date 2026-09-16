use super::*;
use std::{sync::Arc, time::Duration};

mod fixture;
mod network_contract;
use fixture::*;

#[tokio::test]
async fn exact_bound_grant_returns_bytes_only_after_scan_and_final_authority_check() {
    let fixture = Fixture::new();
    let result = fixture.run().await.unwrap();
    assert_eq!(&*result.bytes, PNG);
    assert_eq!(result.mime, Mime::Png);
    assert_eq!(result.sha256, format!("{:x}", Sha256::digest(PNG)));
    assert!(result.source == request().source);
    assert_eq!(
        fixture.events(),
        [
            "authorize",
            "revalidate",
            "resolve",
            "connect",
            "body",
            "body",
            "body",
            "scan",
            "revalidate",
            "body_dropped"
        ]
    );
    assert_eq!(
        fixture.http.targets.lock().unwrap().as_slice(),
        &[vec!["8.8.8.8:443".parse::<SocketAddr>().unwrap()]]
    );
}

#[tokio::test]
async fn production_authority_never_resolves_or_fetches_callback_references() {
    let f = Fixture::new();
    assert_eq!(
        retrieve(
            &request(),
            &UnavailableAuthority,
            &f.dns,
            &f.http,
            &f.scanner
        )
        .await
        .err(),
        Some(Unavailable::Authorization)
    );
    assert!(f.events().is_empty());
}

#[tokio::test]
async fn changed_owner_bridge_bot_platform_source_or_reference_cannot_use_another_grant() {
    for field in 0..8 {
        let mut f = Fixture::new();
        match field {
            0 => {
                f.authority.request.source.owner =
                    layer2_core::resource_access::ResourceOwner::Person {
                        subject: layer2_core::SubjectId::new("other"),
                    }
            }
            1 => f.authority.request.source.bridge.bridge_id.0 = "brg_other".into(),
            2 => f.authority.request.source.bridge.config_revision += 1,
            3 => f.authority.request.source.bot_id = "other-bot".into(),
            4 => f.authority.request.source.platform = layer2_core::bridge::BridgePlatform::Feishu,
            5 => f.authority.request.source.source_id = "other-source".into(),
            6 => f.authority.request.reference.image_key = Some("other-key".into()),
            7 => f.authority.request.reference.url = "https://media.example/other".into(),
            _ => unreachable!(),
        }
        assert_eq!(
            f.run().await.err(),
            Some(Unavailable::Authorization),
            "{field}"
        );
        assert_eq!(f.events(), ["authorize"]);
    }
}

#[tokio::test]
async fn first_or_final_authority_refusal_releases_no_content() {
    for check in [1, 2] {
        let mut f = Fixture::new();
        f.authority.fail_check = Some(check);
        assert_eq!(f.run().await.err(), Some(Unavailable::Authorization));
        assert_eq!(f.events().contains(&"scan"), check == 2);
        assert_eq!(f.events().contains(&"connect"), check == 2);
    }
}

#[tokio::test]
async fn rejected_unavailable_and_missing_scanners_release_no_content() {
    for outcome in [ScanOutcome::Rejected, ScanOutcome::Unavailable] {
        let mut f = Fixture::new();
        f.scanner.outcome = outcome;
        assert_eq!(f.run().await.err(), Some(Unavailable::Scan));
        assert_eq!(f.events().iter().filter(|e| **e == "revalidate").count(), 1);
    }
    let f = Fixture::new();
    assert_eq!(
        retrieve(
            &request(),
            &f.authority,
            &f.dns,
            &f.http,
            &UnavailableScanner
        )
        .await
        .err(),
        Some(Unavailable::Scan)
    );
}

#[tokio::test]
async fn redirects_auth_failures_and_encoded_bodies_are_refused_before_reading() {
    for status in [201, 204, 206, 301, 302, 303, 307, 308, 401, 403, 404, 500] {
        let mut f = Fixture::new();
        f.http.status = status;
        assert_eq!(f.run().await.err(), Some(Unavailable::Response), "{status}");
        assert!(!f.events().contains(&"body"));
    }
    for encoding in ["gzip", "br", "deflate", "identity", "unknown"] {
        let mut f = Fixture::new();
        f.http.encoding = Some(encoding.into());
        assert_eq!(f.run().await.err(), Some(Unavailable::Response));
        assert!(!f.events().contains(&"body"));
    }
}

#[tokio::test]
async fn streamed_bounds_do_not_trust_declared_size_or_wait_for_complete_body() {
    for length in [None, Some(100)] {
        let mut f = Fixture::new();
        f.http.length = length;
        f.http.chunks = vec![
            Ok(vec![0; 100]),
            Ok(vec![0; 29]),
            Err(Unavailable::Transport),
        ];
        assert_eq!(f.run().await.err(), Some(Unavailable::Bounds));
        assert_eq!(f.events().iter().filter(|e| **e == "body").count(), 2);
        assert_eq!(f.http.body_budgets.lock().unwrap().as_slice(), &[128, 28]);
        assert!(!f.events().contains(&"scan"));
    }
    for length in [Some(0), Some(129), Some(u64::MAX)] {
        let mut f = Fixture::new();
        f.http.length = length;
        assert_eq!(f.run().await.err(), Some(Unavailable::Bounds));
        assert!(!f.events().contains(&"body"));
    }
}

#[tokio::test]
async fn truncated_or_overdeclared_body_and_stream_failure_release_no_bytes() {
    for delta in [-1, 1] {
        let mut f = Fixture::new();
        f.http.length = Some((PNG.len() as i64 + delta) as u64);
        assert_eq!(f.run().await.err(), Some(Unavailable::Bounds));
        assert!(!f.events().contains(&"scan"));
    }
    let mut f = Fixture::new();
    f.http.length = None;
    f.http.chunks.clear();
    assert_eq!(f.run().await.err(), Some(Unavailable::Bounds));
    let mut f = Fixture::new();
    f.http.chunks.push(Err(Unavailable::Transport));
    assert_eq!(f.run().await.err(), Some(Unavailable::Transport));
    assert!(!f.events().contains(&"scan"));
}

#[tokio::test]
async fn unknown_or_mismatched_mime_and_unverified_signature_never_reach_scanner() {
    for mime in [
        None,
        Some("text/html"),
        Some("image/jpeg"),
        Some("application/octet-stream"),
        Some("image/png; charset=utf-8"),
    ] {
        let mut f = Fixture::new();
        f.http.mime = mime.map(str::to_owned);
        assert_eq!(f.run().await.err(), Some(Unavailable::Mime));
        assert!(!f.events().contains(&"body"));
    }
    for body in [
        b"<html>not a PNG</html>".as_slice(),
        b"%PDF-1.7",
        &[0xff, 0xd8, 0xff],
    ] {
        let mut f = Fixture::new();
        f.http.length = None;
        f.http.chunks = vec![Ok(body.to_vec())];
        assert_eq!(f.run().await.err(), Some(Unavailable::Mime));
        assert!(!f.events().contains(&"scan"));
    }
}

#[tokio::test]
async fn exact_limit_and_missing_content_length_remain_bounded() {
    let mut f = Fixture::new();
    f.authority.limit = PNG.len();
    f.http.length = None;
    assert_eq!(&*f.run().await.unwrap().bytes, PNG);
}

#[tokio::test(start_paused = true)]
async fn authorization_resolution_connection_stream_and_scan_share_one_deadline() {
    for stage in 0..6 {
        let mut f = Fixture::new();
        let delay = Duration::from_secs(16);
        match stage {
            0 => f.authority.authorize_delay = delay,
            1 => f.dns.delay = delay,
            2 => f.http.connect_delay = delay,
            3 => f.http.body_delay = delay,
            4 => f.scanner.delay = delay,
            5 => f.authority.check_delay = delay,
            _ => unreachable!(),
        }
        let started = Instant::now();
        assert_eq!(
            f.run().await.err(),
            Some(Unavailable::Deadline),
            "stage {stage}"
        );
        assert!(Instant::now() - started <= MAX_DURATION);
    }
}

#[tokio::test(start_paused = true)]
async fn slow_authorization_and_dns_cannot_start_transport_at_attempt_expiry() {
    let mut f = Fixture::new();
    f.authority.authorize_delay = Duration::from_secs(10);
    f.authority.ttl = Duration::from_secs(10);
    f.dns.delay = Duration::from_secs(5);
    let started = Instant::now();
    assert_eq!(f.run().await.err(), Some(Unavailable::Deadline));
    assert_eq!(Instant::now() - started, MAX_DURATION);
    assert!(f.events().contains(&"resolve"));
    assert!(!f.events().contains(&"connect"));
}

#[tokio::test(start_paused = true)]
async fn transport_keeps_the_attempt_deadline_after_a_slow_grant_issuer() {
    let mut f = Fixture::new();
    f.authority.authorize_delay = Duration::from_secs(10);
    f.authority.ttl = Duration::from_secs(10);
    f.dns.delay = Duration::from_secs(2);
    let started = Instant::now();
    assert!(f.run().await.is_ok());
    assert_eq!(
        f.http.deadlines.lock().unwrap().as_slice(),
        &[started + MAX_DURATION]
    );
}

#[tokio::test(start_paused = true)]
async fn scan_and_final_revalidation_cannot_renew_elapsed_grant_time() {
    let mut f = Fixture::new();
    f.authority.ttl = Duration::from_secs(3);
    f.authority.check_delay = Duration::from_secs(1);
    f.scanner.delay = Duration::from_secs(2);
    assert_eq!(f.run().await.err(), Some(Unavailable::Deadline));
    let mut f = Fixture::new();
    f.authority.ttl = Duration::from_secs(3);
    f.authority.check_delay = Duration::from_secs(1);
    f.scanner.delay = Duration::from_millis(1500);
    assert_eq!(f.run().await.err(), Some(Unavailable::Deadline));
    assert_eq!(f.events().iter().filter(|e| **e == "revalidate").count(), 2);
    let mut f = Fixture::new();
    f.authority.authorize_delay = Duration::from_secs(10);
    f.scanner.delay = Duration::from_secs(6);
    let started = Instant::now();
    assert_eq!(f.run().await.err(), Some(Unavailable::Deadline));
    assert_eq!(Instant::now() - started, MAX_DURATION);
}

#[test]
fn mime_signatures_are_not_treated_as_scanning_or_format_validation() {
    assert!(Mime::Jpeg.corroborates(&[0xff, 0xd8, 0xff]));
    assert!(Mime::Pdf.corroborates(b"%PDF-1.7"));
    assert!(!Mime::Png.corroborates(b"PNG"));
}

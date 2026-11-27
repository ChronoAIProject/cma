use super::*;
use serde_json::{json, Value};
fn selection(metric: SourceMetric) -> Selection {
    let range: Value =
        serde_json::from_str(include_str!("fixtures/live-source-range.json")).unwrap();
    Selection {
        cluster: "example-cluster-01".into(),
        namespace: "example-registry-01".into(),
        names: vec![if metric.is_pvc() {
            "home-fixture-cold"
        } else {
            "fixture-cold"
        }
        .into()],
        metric,
        from: range["from"].as_u64().unwrap(),
        to: range["to"].as_u64().unwrap(),
    }
}
#[test]
fn measured_production_fixture_preserves_all_four_raw_sources() {
    let rows: Vec<Value> = include_str!("fixtures/live-source.jsonl")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    for metric in SourceMetric::ALL {
        let row = rows
            .iter()
            .find(|r| r["metric"]["__name__"] == metric.name())
            .unwrap();
        let decoded = decode_export(&serde_json::to_vec(row).unwrap(), &selection(metric)).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].samples.len(), 10);
        assert!(decoded[0]
            .samples
            .iter()
            .all(|s| s.value.is_some_and(|value| value > 0.)));
        assert!(decoded[0]
            .samples
            .windows(2)
            .all(|p| (p[1].at - p[0].at - 30.).abs() < 1.));
    }
}
#[test]
fn duplicate_scrapes_and_invalid_source_shapes_cannot_inflate_values() {
    let original: Value = serde_json::from_str(
        include_str!("fixtures/live-source.jsonl")
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    for mutation in ["replica", "timestamp", "job", "container", "cadvisor"] {
        let mut changed = original.clone();
        match mutation {
            "replica" => changed["metric"]["replica"] = json!("second"),
            "timestamp" => changed["timestamps"][0] = json!(1),
            "job" => changed["metric"]["job"] = json!("duplicate-kubelet"),
            "container" => changed["metric"]["container"] = json!("pause"),
            "cadvisor" => changed["metric"]["metrics_path"] = json!("/metrics/cadvisor"),
            _ => unreachable!(),
        }
        let bytes = format!("{original}\n{changed}\n");
        assert!(
            decode_export(bytes.as_bytes(), &selection(SourceMetric::CpuCounter)).is_err(),
            "{mutation}"
        );
    }
    let mut many = selection(SourceMetric::CpuCounter);
    many.names = vec!["cold".into(); 101];
    assert!(selector(&many).is_err());
}
#[tokio::test]
async fn fixed_proxy_get_rejects_http_errors_and_partial_source_responses() {
    for (status, body) in [
        (503, "unavailable"),
        (200, r#"{"status":"success","isPartial":true}"#),
        (200, "truncated{"),
    ] {
        let server = tower::service_fn(move |r: http::Request<kube::client::Body>| async move {
            assert_eq!(r.method(), http::Method::GET);
            assert!(r
                .uri()
                .path()
                .ends_with("/select/0/prometheus/api/v1/export"));
            assert!(r.uri().query().unwrap().contains("deny_partial_response=1"));
            Ok::<_, std::convert::Infallible>(
                http::Response::builder()
                    .status(status)
                    .body(kube::client::Body::from(body.as_bytes().to_vec()))
                    .unwrap(),
            )
        });
        let client = MetricsClient::new(MetricsTransport::KubernetesProxy(kube::Client::new(
            server,
            "monitoring",
        )));
        assert!(client
            .export(&selection(SourceMetric::CpuCounter))
            .await
            .is_err());
    }
}

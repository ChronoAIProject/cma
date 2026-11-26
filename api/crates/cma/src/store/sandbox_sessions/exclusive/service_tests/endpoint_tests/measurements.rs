//! Matched public connection windows; diagnostics remain a separate execution.
use super::*;
use crate::auth::desktop_test_support::ProviderSnapshot;
use serde_json::{json, Value};

struct Diagnostics {
    handle: metrics_exporter_prometheus::PrometheusHandle,
}

#[derive(Clone, Copy, Default)]
struct Resolution {
    count: u64,
    seconds: f64,
}

impl Diagnostics {
    fn start() -> Self {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::set_global_recorder(recorder).expect("diagnostics need an isolated test process");
        Self { handle }
    }

    fn resolution(&self) -> Resolution {
        let mut result = Resolution::default();
        for line in self.handle.render().lines() {
            if !line.contains("path=\"ambient\"") {
                continue;
            }
            if line.starts_with("cma_auth_resolution_seconds_sum{") {
                result.seconds += line
                    .split_whitespace()
                    .last()
                    .unwrap()
                    .parse::<f64>()
                    .unwrap();
            }
            if line.starts_with("cma_auth_resolution_seconds_count{") {
                result.count += line
                    .split_whitespace()
                    .last()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
            }
        }
        result
    }
}

fn resolver_delta(before: Resolution, after: Resolution) -> Value {
    json!({"count": after.count - before.count, "milliseconds": (after.seconds - before.seconds) * 1000.0})
}

fn provider_delta(before: ProviderSnapshot, after: ProviderSnapshot) -> Value {
    let endpoints = [
        ("discovery", before.discovery, after.discovery),
        ("userinfo", before.userinfo, after.userinfo),
        ("introspection", before.introspection, after.introspection),
    ];
    Value::Object(
        endpoints
            .into_iter()
            .map(|(name, before, after)| {
                (
                    name.into(),
                    json!({
                        "requests": after.requests - before.requests,
                        "completed": after.completed - before.completed,
                        "timed": after.timed - before.timed,
                        "handler_elapsed_nanos": after.elapsed_nanos - before.elapsed_nanos,
                    }),
                )
            })
            .collect(),
    )
}

async fn attempt(f: &Fixture, ingress: usize, diagnostics: Option<&Diagnostics>) -> Value {
    let provider_before = f.auth.provider_snapshot();
    let original_before = diagnostics.map(Diagnostics::resolution).unwrap_or_default();
    let mut after_discovery = None;
    let mut after_upgrade = None;
    let mut websocket_status = None;
    let mut socket = None;
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let response = f.ingresses[ingress]
            .discovery_result(&f.resources.agents[ingress], &f.auth.headers)
            .await;
        after_discovery = diagnostics.map(Diagnostics::resolution);
        let response = response.map_err(|_| "discovery_transport")?;
        if response.status() != 200 {
            return Err("discovery_refusal");
        }
        let descriptor: Value = response.json().await.map_err(|_| "descriptor_body")?;
        if descriptor["websocket_url"]
            != format!("/api/v1/agents/{}/desktop/ws", f.resources.agents[ingress])
        {
            return Err("descriptor_shape");
        }
        let connected = f.connect(ingress).await;
        after_upgrade = diagnostics.map(Diagnostics::resolution);
        socket = Some(match connected {
            Ok(socket) => socket,
            Err(error) => {
                if let Error::Http(response) = error {
                    websocket_status = Some(response.status().as_u16());
                }
                return Err("websocket_refusal_or_transport");
            }
        });
        let connection = socket.as_mut().unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            connection
                .send(Message::Binary(vec![19, 62]))
                .await
                .map_err(|_| "frame_send")?;
            match connection.next().await {
                Some(Ok(Message::Binary(bytes))) if bytes == [19, 62] => Ok(()),
                _ => Err("frame_reply"),
            }
        })
        .await
        .map_err(|_| "frame_deadline")?
    })
    .await
    .unwrap_or(Err("attempt_deadline"));
    let milliseconds = started.elapsed().as_secs_f64() * 1000.0;
    let provider_after = f.auth.provider_snapshot();
    if let Some(mut socket) = socket {
        let _ = tokio::time::timeout(Duration::from_secs(2), socket.close(None)).await;
    }
    let mut sample = json!({
        "milliseconds": milliseconds,
        "outcome": result.err().unwrap_or("ok"),
        "ingress": ingress,
        "websocket_refusal_status": websocket_status,
    });
    if diagnostics.is_some() {
        sample["discovery_original_resolver"] = after_discovery
            .map(|after| resolver_delta(original_before, after))
            .unwrap_or(Value::Null);
        sample["websocket_original_resolver"] = after_discovery
            .zip(after_upgrade)
            .map(|(before, after)| resolver_delta(before, after))
            .unwrap_or(Value::Null);
        sample["provider"] = provider_delta(provider_before, provider_after);
    }
    sample
}

#[tokio::test]
#[ignore = "requires separate matched desktop-replica fixture windows and preregistered noise"]
async fn matched_public_desktop_connect_windows() {
    let mode = std::env::var("CMA_DESKTOP_PUBLIC_MODE").expect("explicit measurement mode");
    assert!(["one", "two"].contains(&mode.as_str()));
    let diagnostic = std::env::var("CMA_DESKTOP_PUBLIC_PHASES").as_deref() == Ok("1");
    let subscriber = diagnostic.then(|| {
        tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_env_filter("cma::desktop::connect=debug")
            .finish()
    });
    let _scope = subscriber.map(tracing::subscriber::set_default);
    let diagnostics = diagnostic.then(Diagnostics::start);
    let f = Fixture::new(true).await;
    f.auth.set_provider_timing(diagnostic);
    let mut samples = Vec::new();
    for window in 0..3 {
        for ordinal in 0..if diagnostic { 15 } else { 105 } {
            let ingress = usize::from(mode == "two" && ordinal % 2 == 1);
            println!(
                "DESKTOP_PUBLIC_START={}",
                json!({"mode":mode,"window":window,"ordinal":ordinal})
            );
            let mut sample = attempt(&f, ingress, diagnostics.as_ref()).await;
            sample["window"] = json!(window);
            sample["ordinal"] = json!(ordinal);
            sample["warmup"] = json!(ordinal < 5);
            sample["cold"] = json!(window == 0 && ordinal == 0);
            println!("DESKTOP_PUBLIC_SAMPLE={sample}");
            samples.push(sample);
            // Settlement is outside the measured boundary, before the next
            // attempt. Real refusal/timeout stays in samples even if it recovers.
            f.available(ingress, Duration::from_secs(18)).await;
        }
    }
    println!(
        "DESKTOP_PUBLIC_MEASUREMENT={}",
        json!({
            "schema":1, "mode":mode, "diagnostics":diagnostic,
            "boundary":"authenticated public discovery through first returned synthetic echo frame",
            "replicas":"independent routers, AuthStates, resource/auth Mongo clients and concrete adapters in one CMA process",
            "receiver":"actual authenticated receiver child with synthetic display",
            "provider":"local synthetic IdP and authenticated provider proxy",
            "samples":samples,
        })
    );
    f.cleanup().await;
}

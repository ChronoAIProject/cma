//! Paired relay measurement of ingress receipt to the successful provider write.
use super::app::App;
use serde_json::{json, Value};

const WARMUPS: usize = 20;
const SAMPLES: usize = 500;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "measures 500 warm samples per path against the 2 ms routing budget"]
async fn routed_single_target_adds_at_most_two_ms_p99_before_provider_write() {
    cmaeg::init_logging();
    let mut app = App::new(true, |raw| {
        raw["reply"]["auto_final"] = false.into();
        raw["reply"]["sinks"] = json!([{"type":"source"}]);
    })
    .await;
    let mut raw = app.channel.definition.to_blob_json();
    raw["record_id"] = uuid::Uuid::new_v4().to_string().into();
    raw.as_object_mut().unwrap().remove("routing");
    raw["secrets"]
        .as_object_mut()
        .unwrap()
        .remove("consumer_keys");
    let legacy = app.seed(raw).await;
    let channels = [&legacy, &app.channel];
    let mut samples = [Vec::new(), Vec::new()];
    for index in 0..WARMUPS + SAMPLES {
        for arm in if index % 2 == 0 { [0, 1] } else { [1, 0] } {
            let response = app
                .callback_to(
                    channels[arm],
                    "ordinary input",
                    &format!("pair-{index}-{arm}"),
                )
                .await;
            let launch = measure(response).await;
            app.quiet().await;
            if index >= WARMUPS {
                samples[arm].push(launch);
            }
        }
    }
    app.drain().await;
    let turns = app.turns(0);
    assert_eq!(turns.len(), 2 * (WARMUPS + SAMPLES));
    for arm in 0..2 {
        let observed: Vec<_> = turns
            .iter()
            .filter(|turn| {
                turn["event_context"]["activity"]["event_id"]
                    .as_str()
                    .unwrap()
                    .ends_with(&format!("-{arm}"))
            })
            .collect();
        assert_eq!(observed.len(), WARMUPS + SAMPLES);
        assert!(observed
            .iter()
            .all(|turn| turn["event_context"]["activity"].get("routing").is_some() == (arm == 1)));
    }
    assert!(app.turns(1).is_empty());
    assert!(app.peers.chat_calls().is_empty());
    let delta: Vec<_> = samples[1]
        .iter()
        .zip(&samples[0])
        .map(|(routed, legacy)| routed - legacy)
        .collect();
    let legacy = percentile(&samples[0], 99);
    let routed = percentile(&samples[1], 99);
    println!("relay ingress-to-provider-write: count/path={SAMPLES}, warmups/path={WARMUPS}, legacy_p99_ms={legacy:.3}, routed_p99_ms={routed:.3}, added_p99_ms={:.3}, paired_delta_p50_ms={:.3}, paired_delta_p99_ms={:.3}",
        routed - legacy, percentile(&delta, 50), percentile(&delta, 99));
    assert!(
        routed - legacy <= 2.0,
        "routed p99 {routed:.3} ms minus legacy p99 {legacy:.3} ms exceeds 2 ms"
    );
}

async fn measure(response: reqwest::Response) -> f64 {
    let status = response.status();
    let header = response
        .headers()
        .get("server-timing")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let body: Value = response.json().await.unwrap();
    assert_eq!(status, 202, "{body}");
    assert_eq!(body["status"], "accepted");
    assert!(!header.contains("not_written"), "{header}");
    let launch: f64 = header
        .split(',')
        .find_map(|part| part.trim().strip_prefix("launch;dur="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(launch.is_finite() && launch > 0.0, "{header}");
    launch
}
fn percentile(samples: &[f64], percent: usize) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[(sorted.len() * percent).div_ceil(100) - 1]
}

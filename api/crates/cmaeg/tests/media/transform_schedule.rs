//! The one transform slot: whole sets, audio first, per-channel FIFOs and
//! fair interleaving; and the pure parsing of probe output and PCM.
use super::{
    budget::channel,
    transform::{audio, describe, jpeg, keyframes, probe_json, Harness, Step},
};
use bytes::BytesMut;
use cmaeg::media::transform::{
    argv::JobOp,
    launcher::JobSpec,
    outcome::TransformOutcome,
    pcm::{pcm_seconds, write_wav_header},
    probe_json::ProbeOutput,
    scheduler::{SchedulerSnapshot, SetClass, TransformScheduler, AUDIO_SET_WALL},
    MediaTransformer, TransformError,
};
use std::time::Duration;
use tokio::{task::JoinHandle, time::Instant};

const SECOND: Duration = Duration::from_secs(1);

/// Every job takes `runs_for`; probes report a 4 s video.
fn steady(runs_for: Duration) -> impl Fn(&JobSpec) -> Step {
    move |spec| match &spec.op {
        JobOp::Probe => Step::ok(runs_for, probe_json("4")),
        JobOp::Frame(_) => Step::ok(runs_for, jpeg(9)),
        _ => Step::ok(runs_for, vec![0; 64]),
    }
}

fn spawn_audio(harness: &Harness, name: &str, input: &str) -> JoinHandle<bool> {
    let transformer = harness.transformer.clone();
    let request = audio(name, input, 5);
    tokio::spawn(async move { transformer.audio(request).await.is_ok() })
}

fn spawn_keyframes(harness: &Harness, name: &str, frames: u8) -> JoinHandle<bool> {
    let transformer = harness.transformer.clone();
    let request = keyframes(name, frames);
    tokio::spawn(async move { transformer.keyframes(request).await.is_ok() })
}

/// Let spawned sets enqueue in order, one paused millisecond apart.
async fn tick() {
    tokio::time::sleep(Duration::from_millis(1)).await;
}

/// The inputs of the jobs that ran, in order.
fn inputs(harness: &Harness) -> Vec<String> {
    harness
        .ran()
        .iter()
        .map(|ran| String::from_utf8(ran.input.clone()).unwrap())
        .collect()
}

#[tokio::test(start_paused = true)]
async fn slot_is_held_across_a_whole_set() {
    let harness = Harness::new(steady(SECOND));
    let video = spawn_keyframes(&harness, "a", 3);
    tick().await;
    let voice = spawn_audio(&harness, "b", "voice");
    assert!(video.await.unwrap());
    assert!(voice.await.unwrap());
    let ops: Vec<String> = harness.ran().iter().map(|ran| describe(&ran.op)).collect();
    // The audio set, though it has priority, waits for the whole keyframe set.
    assert_eq!(
        ops,
        [
            "probe",
            "frame 0.667",
            "frame 2.000",
            "frame 3.333",
            "audio 5"
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn audio_is_served_before_keyframes() {
    let harness = Harness::new(steady(SECOND));
    let first = spawn_audio(&harness, "a", "first");
    tick().await;
    let video = spawn_keyframes(&harness, "b", 1);
    tick().await;
    let voice = spawn_audio(&harness, "c", "voice");
    for set in [first, video, voice] {
        assert!(set.await.unwrap());
    }
    assert_eq!(inputs(&harness), ["first", "voice", "b", "b"]);
}

#[tokio::test(start_paused = true)]
async fn same_channel_sets_wait_fifo_and_are_never_refused() {
    // Each voice note takes 15 s: the third waits 30 s in all, past the 20 s
    // audio deadline, which starts only when it enters the global queue.
    let harness = Harness::new(steady(Duration::from_secs(15)));
    let started = Instant::now();
    let notes: Vec<_> = ["one", "two", "three"]
        .into_iter()
        .map(|input| spawn_audio(&harness, "same", input))
        .collect();
    for note in notes {
        assert!(note.await.unwrap());
    }
    assert_eq!(inputs(&harness), ["one", "two", "three"]);
    let starts: Vec<Duration> = harness
        .ran()
        .iter()
        .map(|ran| ran.started - started)
        .collect();
    assert_eq!(
        starts,
        [
            Duration::ZERO,
            Duration::from_secs(15),
            Duration::from_secs(30)
        ]
    );
    assert_eq!(
        harness.ran()[2].deadline,
        started + Duration::from_secs(30) + AUDIO_SET_WALL
    );
}

#[tokio::test(start_paused = true)]
async fn other_channels_interleave_fairly() {
    let harness = Harness::new(steady(SECOND));
    let mut sets = Vec::new();
    for input in ["a1", "a2", "a3"] {
        sets.push(spawn_audio(&harness, "a", input));
    }
    for input in ["b1", "b2", "b3"] {
        sets.push(spawn_audio(&harness, "b", input));
    }
    for set in sets {
        assert!(set.await.unwrap());
    }
    assert_eq!(inputs(&harness), ["a1", "b1", "a2", "b2", "a3", "b3"]);
}

#[tokio::test(start_paused = true)]
async fn a_set_that_cannot_reach_the_slot_in_time_fails() {
    // A keyframe set holds the slot for 25 s; a voice note queued behind it
    // gives up at its 20 s deadline without running.
    let harness = Harness::new(|spec: &JobSpec| match &spec.op {
        JobOp::Probe => Step::ok(SECOND, probe_json("4")),
        _ => Step::ok(Duration::from_secs(12), jpeg(9)),
    });
    let started = Instant::now();
    let video = spawn_keyframes(&harness, "a", 2);
    tick().await;
    let late = harness.transformer.audio(audio("d", "late", 5)).await;
    assert_eq!(
        late.unwrap_err(),
        TransformError::job(TransformOutcome::Timeout)
    );
    assert_eq!(
        Instant::now(),
        started + Duration::from_millis(1) + AUDIO_SET_WALL
    );
    assert!(video.await.unwrap());
    assert!(!inputs(&harness).contains(&"late".to_owned()));
}

#[tokio::test(start_paused = true)]
async fn a_cancelled_waiter_leaves_and_the_slot_moves_on() {
    let scheduler = TransformScheduler::new();
    let held = scheduler
        .acquire(&channel("a"), SetClass::Keyframes)
        .await
        .unwrap();
    let waiting = {
        let scheduler = scheduler.clone();
        tokio::spawn(async move {
            scheduler
                .acquire(&channel("b"), SetClass::Keyframes)
                .await
                .is_ok()
        })
    };
    let behind = {
        let scheduler = scheduler.clone();
        tokio::spawn(async move {
            scheduler
                .acquire(&channel("a"), SetClass::Audio)
                .await
                .is_ok()
        })
    };
    tick().await;
    assert_eq!(
        scheduler.snapshot(),
        SchedulerSnapshot {
            held: true,
            queued: 1,
            behind_channels: 1
        }
    );
    waiting.abort();
    assert!(waiting.await.unwrap_err().is_cancelled());
    assert_eq!(scheduler.snapshot().queued, 0);
    drop(held);
    assert!(behind.await.unwrap());
    assert_eq!(
        scheduler.snapshot(),
        SchedulerSnapshot {
            held: false,
            queued: 0,
            behind_channels: 0
        }
    );
    assert!(scheduler
        .acquire(&channel("c"), SetClass::Audio)
        .await
        .is_ok());
}

#[test]
fn probe_json_ignores_unknown_fields() {
    let output = ProbeOutput::parse(
        br#"{"programs":[{"id":1}],"stream_groups":[],"streams":[{"codec_type":"audio","index":0},{"codec_type":"video"}],"format":{"duration":"12.480000","bit_rate":"1"},"extra":true}"#,
    )
    .unwrap();
    assert_eq!(output.duration_seconds(), Some(12.48));
    assert_eq!(output.streams.len(), 2);
    assert_eq!(output.streams[1].codec_type.as_deref(), Some("video"));
    assert!(ProbeOutput::parse(b"{}")
        .unwrap()
        .duration_seconds()
        .is_none());
    assert!(ProbeOutput::parse(b"[1]").is_none());
    assert!(ProbeOutput::parse(br#"{"streams":"x"}"#).is_none());
}

#[test]
fn probe_duration_validation() {
    let duration = |value: &str| {
        ProbeOutput::parse(probe_json(value).as_bytes())
            .unwrap()
            .duration_seconds()
    };
    for unknown in ["NaN", "inf", "-inf", "0", "-1", "86400.5", "N/A", ""] {
        assert_eq!(duration(unknown), None, "{unknown}");
    }
    assert_eq!(duration("86400"), Some(86_400.0));
    assert_eq!(duration("0.04"), Some(0.04));
}

#[test]
fn wav_header_is_exact() {
    let mut buffer = BytesMut::zeroed(44 + 4);
    write_wav_header(&mut buffer, 32_000);
    let golden: [u8; 44] = [
        b'R', b'I', b'F', b'F', 0x24, 0x7D, 0x00, 0x00, b'W', b'A', b'V', b'E', b'f', b'm', b't',
        b' ', 0x10, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x80, 0x3E, 0x00, 0x00, 0x00, 0x7D,
        0x00, 0x00, 0x02, 0x00, 0x10, 0x00, b'd', b'a', b't', b'a', 0x00, 0x7D, 0x00, 0x00,
    ];
    assert_eq!(buffer[..44], golden);
    // The bytes after the header are left untouched.
    assert_eq!(buffer[44..], [0; 4]);
}

#[test]
fn pcm_is_metered_by_exact_seconds_rounded_up() {
    assert_eq!(pcm_seconds(0), 0);
    assert_eq!(pcm_seconds(2), 1);
    assert_eq!(pcm_seconds(32_000), 1);
    assert_eq!(pcm_seconds(32_002), 2);
    assert_eq!(pcm_seconds(600 * 32_000), 600);
}

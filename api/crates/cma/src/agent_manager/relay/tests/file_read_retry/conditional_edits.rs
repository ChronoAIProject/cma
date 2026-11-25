use super::*;
use base64::Engine as _;
use serde_json::json;

const EDIT_BYTES: &[u8] = b"conditional edit fixture\n";

impl Steady {
    async fn edit(&self) -> (u16, Value) {
        let response = self
            .http
            .post(format!(
                "{}/api/v1/agents/{}/files",
                self.owner.address, self.agent.id
            ))
            .json(&json!({
                "path": FILE_PATH,
                "expected_sha256": digest(),
                "content_base64": base64::engine::general_purpose::STANDARD.encode(EDIT_BYTES),
            }))
            .send()
            .await
            .unwrap();
        let status = response.status().as_u16();
        (status, response.json().await.unwrap())
    }
}

struct Scenario {
    name: &'static str,
    challenges: &'static [Challenge],
    lose_confirmation: bool,
    lose_edit_reply: bool,
    challenges_seen: usize,
    confirmations: usize,
    dispatched_edits: usize,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn conditional_edit_retries_only_challenge_and_dispatches_at_most_once() {
    use Challenge::{Accept, Refuse, ReplacePod};

    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    for scenario in [
        Scenario {
            name: "status challenge fails first",
            challenges: &[Refuse, Accept, Accept],
            lose_confirmation: false,
            lose_edit_reply: false,
            challenges_seen: 3,
            confirmations: 2,
            dispatched_edits: 1,
        },
        Scenario {
            name: "edit challenge fails first",
            challenges: &[Accept, Refuse, Accept],
            lose_confirmation: false,
            lose_edit_reply: false,
            challenges_seen: 3,
            confirmations: 2,
            dispatched_edits: 1,
        },
        Scenario {
            name: "status challenge consumes allowance",
            challenges: &[Refuse, Accept, Refuse],
            lose_confirmation: false,
            lose_edit_reply: false,
            challenges_seen: 3,
            confirmations: 1,
            dispatched_edits: 0,
        },
        Scenario {
            name: "uncertain confirmation",
            challenges: &[Accept],
            lose_confirmation: true,
            lose_edit_reply: false,
            challenges_seen: 1,
            confirmations: 1,
            dispatched_edits: 0,
        },
        Scenario {
            name: "receiver replacement",
            challenges: &[Accept, ReplacePod],
            lose_confirmation: false,
            lose_edit_reply: false,
            challenges_seen: 2,
            confirmations: 1,
            dispatched_edits: 0,
        },
        Scenario {
            name: "edit commits and reply is lost",
            challenges: &[Accept, Refuse, Accept],
            lose_confirmation: false,
            lose_edit_reply: true,
            challenges_seen: 3,
            confirmations: 2,
            dispatched_edits: 1,
        },
    ] {
        let runtime = Runtime::start().await;
        let steady = Steady::bind(&uri, &runtime, true).await;
        let _ = steady.authority().await;
        let before = dispatches(&runtime);
        script(&runtime, scenario.challenges.iter().copied());
        let counts = &runtime.counts;
        counts
            .lose_confirmation
            .store(scenario.lose_confirmation, SeqCst);
        counts
            .lose_edit_reply
            .store(scenario.lose_edit_reply, SeqCst);
        let (status, body) = steady.edit().await;
        if scenario.dispatched_edits == 1 && !scenario.lose_edit_reply {
            assert_eq!(status, 200, "{}: {body}", scenario.name);
            assert_eq!(body["sha256"], format!("{:x}", Sha256::digest(EDIT_BYTES)));
        } else {
            assert_eq!(status, 503, "{}: {body}", scenario.name);
        }
        assert_eq!(counts.edit_admissions.load(SeqCst), 1, "{}", scenario.name);
        assert_eq!(
            counts.edits.load(SeqCst),
            scenario.dispatched_edits,
            "{}",
            scenario.name
        );
        assert_eq!(
            since(&runtime, before),
            (scenario.challenges_seen, scenario.confirmations, 0),
            "{}",
            scenario.name
        );
        assert_eq!(
            counts.edited_bytes.lock().unwrap().as_deref(),
            (scenario.dispatched_edits == 1).then_some(EDIT_BYTES),
            "{}",
            scenario.name,
        );
        steady.close().await;
    }
}

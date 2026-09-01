use std::collections::VecDeque;
use std::future::pending;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Mutex, Semaphore};

use super::checkpoint_service_tests::{
    build_service, build_service_with_credentials, fixture, CheckpointFixture,
};
use super::*;
use crate::event::Event;
use crate::store::{
    ChatThreadTitle, ChatTitleClaimMode, ChatTitleClaimRequest, PageRequest, SessionStore,
};
use crate::EventKind;

struct ObservedRequest {
    model: Option<String>,
    owner: String,
    assistant: String,
    bearer: String,
}

enum GeneratorReply {
    Value(String),
    Error,
    Wait(Arc<Semaphore>, String),
    Pending,
}

#[derive(Default)]
struct RecordingGenerator {
    calls: AtomicUsize,
    observed: Mutex<Vec<ObservedRequest>>,
    replies: Mutex<VecDeque<GeneratorReply>>,
}

impl RecordingGenerator {
    fn with_replies(replies: impl IntoIterator<Item = GeneratorReply>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into_iter().collect()),
            ..Self::default()
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ChatTitleGenerator for RecordingGenerator {
    async fn generate(
        &self,
        request: ChatTitleGenerationRequest,
    ) -> Result<String, ChatTitleGenerationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.observed.lock().await.push(ObservedRequest {
            model: request.model().map(str::to_owned),
            owner: request.owner_message().to_owned(),
            assistant: request.assistant_message().to_owned(),
            bearer: request.bearer().to_owned(),
        });
        let reply = self.replies.lock().await.pop_front();
        match reply {
            Some(GeneratorReply::Value(value)) => Ok(value),
            Some(GeneratorReply::Error) | None => Err(ChatTitleGenerationError::unavailable()),
            Some(GeneratorReply::Wait(gate, value)) => {
                let permit = gate.acquire().await.expect("the test gate remains open");
                permit.forget();
                Ok(value)
            }
            Some(GeneratorReply::Pending) => pending().await,
        }
    }
}

async fn append_kinds(fixture: &CheckpointFixture, kinds: Vec<EventKind>) {
    let id = &fixture.threads[0].id;
    let tail = fixture.sessions.event_tail(id).await.unwrap();
    let events = kinds
        .into_iter()
        .enumerate()
        .map(|(offset, kind)| Event {
            session_id: id.clone(),
            seq: tail + offset as u64 + 1,
            kind,
            appended_at: format!("2026-09-01T03:00:{offset:02}Z"),
        })
        .collect::<Vec<_>>();
    fixture.sessions.append_events(&events).await.unwrap();
}

async fn complete_turn(fixture: &CheckpointFixture, owner: &str, assistant: &str) {
    append_kinds(
        fixture,
        vec![
            EventKind::OwnerMessage {
                content: owner.into(),
            },
            EventKind::MessageCommitted {
                content: assistant.into(),
            },
            EventKind::TurnCompleted,
        ],
    )
    .await;
}

async fn durable_title(fixture: &CheckpointFixture) -> Option<String> {
    fixture
        .sessions
        .get(&fixture.threads[0].id)
        .await
        .unwrap()
        .unwrap()
        .chat_origin
        .unwrap()
        .title
}

#[tokio::test]
async fn the_first_completed_turn_generates_once_and_refreshes_the_public_projection() {
    let fixture = fixture("title-first-turn", 1).await;
    let id = fixture.threads[0].id.clone();
    let generator = RecordingGenerator::with_replies([GeneratorReply::Value(
        "  Durable Thread Title\nignored private continuation  ".into(),
    )]);

    assert!(fixture
        .service
        .chat_title_candidates(PageRequest::new(16, None))
        .await
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        fixture
            .service
            .generate_chat_title(&id, Some(generator.as_ref()))
            .await
            .unwrap(),
        ChatTitleOutcome::Skipped
    );
    assert_eq!(generator.calls(), 0);

    complete_turn(
        &fixture,
        "How do durable titles work?",
        "They use one fenced write.",
    )
    .await;
    assert_eq!(
        fixture
            .service
            .chat_title_candidates(PageRequest::new(16, None))
            .await
            .unwrap()
            .items
            .as_slice(),
        std::slice::from_ref(&id)
    );
    assert_eq!(
        fixture
            .service
            .generate_chat_title(&id, Some(generator.as_ref()))
            .await
            .unwrap(),
        ChatTitleOutcome::Generated
    );
    assert_eq!(
        durable_title(&fixture).await.as_deref(),
        Some("Durable Thread Title")
    );
    assert_eq!(
        fixture
            .service
            .get_session(&fixture.caller, &id)
            .await
            .unwrap()
            .chat
            .unwrap()
            .title
            .as_deref(),
        Some("Durable Thread Title")
    );

    complete_turn(&fixture, "A later question", "A later response").await;
    assert_eq!(
        fixture
            .service
            .generate_chat_title(&id, Some(generator.as_ref()))
            .await
            .unwrap(),
        ChatTitleOutcome::Skipped
    );
    assert_eq!(generator.calls(), 1);
    assert!(fixture
        .service
        .chat_title_candidates(PageRequest::new(16, None))
        .await
        .unwrap()
        .items
        .is_empty());
}

#[tokio::test]
async fn injection_shaped_output_remains_one_inert_projection_string() {
    let fixture = fixture("title-inert-projection", 1).await;
    let id = fixture.threads[0].id.clone();
    complete_turn(
        &fixture,
        "Keep hostile output inert",
        "Projection uses structured serialization.",
    )
    .await;
    let injection = r#"Safe"},"status":"terminated","owner":"other"#;
    let generator = RecordingGenerator::with_replies([GeneratorReply::Value(injection.into())]);

    assert_eq!(
        fixture
            .service
            .generate_chat_title(&id, Some(generator.as_ref()))
            .await
            .unwrap(),
        ChatTitleOutcome::Generated
    );
    let session = fixture
        .service
        .get_session(&fixture.caller, &id)
        .await
        .unwrap();
    let projected = serde_json::to_value(session).unwrap();
    assert_eq!(projected["chat"]["title"], injection);
    assert_eq!(projected["status"], "ready");
    assert!(projected.get("owner").is_some());
    assert_eq!(
        projected
            .as_object()
            .unwrap()
            .keys()
            .filter(|key| *key == "status")
            .count(),
        1
    );
}

#[tokio::test]
async fn first_messages_are_utf8_bounded_and_use_the_host_model_and_session_credential() {
    let fixture = fixture("title-private-inputs", 1).await;
    let id = fixture.threads[0].id.clone();
    {
        let mut records = fixture.hosts.records.lock().await;
        let host = records.values_mut().next().unwrap();
        host.pinned_settings.settings.agent.model = Some("host-pinned-model".into());
    }
    let owner = format!(
        "{}{}owner-tail",
        "o".repeat(CHAT_TITLE_INPUT_BYTES - 1),
        "\u{754c}"
    );
    let assistant = format!(
        "{}{}assistant-tail",
        "a".repeat(CHAT_TITLE_INPUT_BYTES - 1),
        "\u{754c}"
    );
    complete_turn(&fixture, &owner, &assistant).await;
    append_kinds(
        &fixture,
        vec![
            EventKind::OwnerMessage {
                content: "later-owner-marker".into(),
            },
            EventKind::MessageCommitted {
                content: "later-assistant-marker".into(),
            },
            EventKind::TurnCompleted,
        ],
    )
    .await;
    let generator =
        RecordingGenerator::with_replies([GeneratorReply::Value("Bounded Private Inputs".into())]);

    assert_eq!(
        fixture
            .service
            .generate_chat_title(&id, Some(generator.as_ref()))
            .await
            .unwrap(),
        ChatTitleOutcome::Generated
    );
    let observed = generator.observed.lock().await;
    let request = observed.first().unwrap();
    assert_eq!(request.model.as_deref(), Some("host-pinned-model"));
    assert_eq!(request.bearer, "checkpoint-credential");
    assert!(request.owner.len() <= CHAT_TITLE_INPUT_BYTES);
    assert!(request.assistant.len() <= CHAT_TITLE_INPUT_BYTES);
    assert!(request.owner.is_char_boundary(request.owner.len()));
    assert!(request.assistant.is_char_boundary(request.assistant.len()));
    assert!(!request.owner.contains("owner-tail"));
    assert!(!request.assistant.contains("assistant-tail"));
    assert!(!request.owner.contains("later-owner-marker"));
    assert!(!request.assistant.contains("later-assistant-marker"));
}

#[tokio::test]
async fn one_failure_is_retried_but_total_failure_writes_the_owner_fallback() {
    let retry_fixture = fixture("title-retry", 1).await;
    complete_turn(
        &retry_fixture,
        "Explain retry policy",
        "One retry is bounded.",
    )
    .await;
    let retry = RecordingGenerator::with_replies([
        GeneratorReply::Error,
        GeneratorReply::Value("Retry Recovered Title".into()),
    ]);
    assert_eq!(
        retry_fixture
            .service
            .generate_chat_title(&retry_fixture.threads[0].id, Some(retry.as_ref()))
            .await
            .unwrap(),
        ChatTitleOutcome::Generated
    );
    assert_eq!(retry.calls(), 2);
    assert_eq!(
        durable_title(&retry_fixture).await.as_deref(),
        Some("Retry Recovered Title")
    );

    let fallback_fixture = fixture("title-total-failure", 1).await;
    let owner = format!("{}\nprivate-owner-second-line", "f".repeat(80));
    complete_turn(&fallback_fixture, &owner, "The model call will fail.").await;
    let failed = RecordingGenerator::with_replies([GeneratorReply::Error, GeneratorReply::Error]);
    assert_eq!(
        fallback_fixture
            .service
            .generate_chat_title(&fallback_fixture.threads[0].id, Some(failed.as_ref()))
            .await
            .unwrap(),
        ChatTitleOutcome::Fallback
    );
    assert_eq!(failed.calls(), 2);
    let title = durable_title(&fallback_fixture).await.unwrap();
    assert_eq!(title, "f".repeat(CHAT_TITLE_FALLBACK_CHARACTERS));
    assert!(!title.contains("private"));
}

#[tokio::test(start_paused = true)]
async fn each_model_attempt_has_a_ten_second_timeout_before_fallback() {
    let fixture = fixture("title-timeout", 1).await;
    complete_turn(
        &fixture,
        "Timeout fallback owner",
        "A response that never arrives",
    )
    .await;
    let generator =
        RecordingGenerator::with_replies([GeneratorReply::Pending, GeneratorReply::Pending]);
    let service = Arc::clone(&fixture.service);
    let id = fixture.threads[0].id.clone();
    let observed = Arc::clone(&generator);
    let task = tokio::spawn(async move {
        service
            .generate_chat_title(&id, Some(observed.as_ref()))
            .await
    });

    wait_for_calls(&generator, 1).await;
    tokio::time::advance(Duration::from_secs(CHAT_TITLE_CALL_TIMEOUT_SECONDS)).await;
    wait_for_calls(&generator, 2).await;
    tokio::time::advance(Duration::from_secs(CHAT_TITLE_CALL_TIMEOUT_SECONDS)).await;
    tokio::task::yield_now().await;

    assert_eq!(task.await.unwrap().unwrap(), ChatTitleOutcome::Fallback);
    assert_eq!(generator.calls(), 2);
    assert_eq!(
        durable_title(&fixture).await.as_deref(),
        Some("Timeout fallback owner")
    );
}

async fn wait_for_calls(generator: &RecordingGenerator, expected: usize) {
    for _ in 0..100 {
        if generator.calls() >= expected {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("generator did not reach {expected} calls");
}

#[tokio::test]
async fn concurrent_replicas_make_only_one_model_call() {
    let fixture = fixture("title-replica-race", 1).await;
    complete_turn(&fixture, "Replica race owner", "Only one worker may spend.").await;
    let second = build_service(
        Arc::clone(&fixture.adapter),
        Arc::clone(&fixture.hosts),
        Arc::clone(&fixture.sessions),
        Arc::clone(&fixture.checkpoints),
    );
    let gate = Arc::new(Semaphore::new(0));
    let generator = RecordingGenerator::with_replies([GeneratorReply::Wait(
        Arc::clone(&gate),
        "Single Replica Winner".into(),
    )]);
    let first_service = Arc::clone(&fixture.service);
    let first_id = fixture.threads[0].id.clone();
    let first_generator = Arc::clone(&generator);
    let first = tokio::spawn(async move {
        first_service
            .generate_chat_title(&first_id, Some(first_generator.as_ref()))
            .await
    });
    wait_for_calls(&generator, 1).await;

    assert_eq!(
        second
            .generate_chat_title(&fixture.threads[0].id, Some(generator.as_ref()))
            .await
            .unwrap(),
        ChatTitleOutcome::Skipped
    );
    assert_eq!(generator.calls(), 1);
    gate.add_permits(1);
    assert_eq!(first.await.unwrap().unwrap(), ChatTitleOutcome::Generated);
    assert_eq!(generator.calls(), 1);
}

#[tokio::test]
async fn expired_claims_are_fallback_only_and_fence_the_stale_worker() {
    let fixture = fixture("title-expired-claim", 1).await;
    let id = fixture.threads[0].id.clone();
    complete_turn(
        &fixture,
        "Lease takeover owner",
        "The successor falls back.",
    )
    .await;
    let first_request =
        ChatTitleClaimRequest::new(id.clone(), "title-holder-a", Duration::from_secs(60));
    let first = fixture
        .sessions
        .acquire_chat_title_claim(&first_request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.mode(), ChatTitleClaimMode::Generate);
    assert!(fixture
        .sessions
        .acquire_chat_title_claim(&ChatTitleClaimRequest::new(
            id.clone(),
            "title-holder-b",
            Duration::from_secs(60),
        ))
        .await
        .unwrap()
        .is_none());

    fixture.sessions.expire_chat_title_claim(&id).await;
    let successor = fixture
        .sessions
        .acquire_chat_title_claim(&ChatTitleClaimRequest::new(
            id,
            "title-holder-b",
            Duration::from_secs(60),
        ))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(successor.mode(), ChatTitleClaimMode::FallbackOnly);
    let stale_title = ChatThreadTitle::new("Stale Generated Title".into()).unwrap();
    let fallback = ChatThreadTitle::new("Lease takeover owner".into()).unwrap();
    assert!(!fixture
        .sessions
        .complete_chat_title(&first, &stale_title)
        .await
        .unwrap());
    assert!(fixture
        .sessions
        .complete_chat_title(&successor, &fallback)
        .await
        .unwrap());
    assert!(!fixture
        .sessions
        .complete_chat_title(&first, &stale_title)
        .await
        .unwrap());
    assert_eq!(
        durable_title(&fixture).await.as_deref(),
        Some("Lease takeover owner")
    );
}

#[tokio::test]
async fn completion_is_idempotent_after_a_committed_response_loss() {
    let fixture = fixture("title-completion-loss", 1).await;
    let id = fixture.threads[0].id.clone();
    complete_turn(
        &fixture,
        "Completion retry owner",
        "The token proves the commit.",
    )
    .await;
    let claim = fixture
        .sessions
        .acquire_chat_title_claim(&ChatTitleClaimRequest::new(
            id,
            "completion-holder",
            Duration::from_secs(60),
        ))
        .await
        .unwrap()
        .unwrap();
    let title = ChatThreadTitle::new("Committed Once Title".into()).unwrap();
    fixture.sessions.lose_next_chat_title_completion_response();
    assert!(fixture
        .sessions
        .complete_chat_title(&claim, &title)
        .await
        .is_err());
    assert!(fixture
        .sessions
        .complete_chat_title(&claim, &title)
        .await
        .unwrap());
    assert_eq!(
        durable_title(&fixture).await.as_deref(),
        Some("Committed Once Title")
    );
}

#[tokio::test]
async fn missing_generator_host_or_session_credential_still_writes_a_fallback() {
    let no_generator = fixture("title-no-generator", 1).await;
    complete_turn(&no_generator, "No generator owner", "Use the fallback.").await;
    assert_eq!(
        no_generator
            .service
            .generate_chat_title(&no_generator.threads[0].id, None)
            .await
            .unwrap(),
        ChatTitleOutcome::Fallback
    );
    assert_eq!(
        durable_title(&no_generator).await.as_deref(),
        Some("No generator owner")
    );

    let no_host = fixture("title-no-host", 1).await;
    complete_turn(&no_host, "No host owner", "Use the fallback.").await;
    no_host.hosts.records.lock().await.clear();
    let unused = RecordingGenerator::with_replies([GeneratorReply::Value("must not run".into())]);
    assert_eq!(
        no_host
            .service
            .generate_chat_title(&no_host.threads[0].id, Some(unused.as_ref()))
            .await
            .unwrap(),
        ChatTitleOutcome::Fallback
    );
    assert_eq!(unused.calls(), 0);
    assert_eq!(
        durable_title(&no_host).await.as_deref(),
        Some("No host owner")
    );

    let no_credential = fixture("title-no-credential", 1).await;
    complete_turn(&no_credential, "No credential owner", "Use the fallback.").await;
    let service = build_service_with_credentials(
        Arc::clone(&no_credential.adapter),
        Arc::clone(&no_credential.hosts),
        Arc::clone(&no_credential.sessions),
        Arc::clone(&no_credential.checkpoints),
        None,
    );
    let unused = RecordingGenerator::with_replies([GeneratorReply::Value("must not run".into())]);
    assert_eq!(
        service
            .generate_chat_title(&no_credential.threads[0].id, Some(unused.as_ref()))
            .await
            .unwrap(),
        ChatTitleOutcome::Fallback
    );
    assert_eq!(unused.calls(), 0);
    assert_eq!(
        durable_title(&no_credential).await.as_deref(),
        Some("No credential owner")
    );
}

#[derive(Clone, Default)]
struct LogBuffer(Arc<StdMutex<Vec<u8>>>);

struct LogWriter(Arc<StdMutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogBuffer {
    type Writer = LogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LogWriter(Arc::clone(&self.0))
    }
}

#[tokio::test]
async fn transcript_title_and_credential_content_never_enter_debug_or_tracing() {
    let fixture = fixture("title-private-observability", 1).await;
    let owner = "owner-private-observability-marker";
    let assistant = "assistant-private-observability-marker";
    let generated = "generated-private-observability-marker";
    complete_turn(&fixture, owner, assistant).await;
    let generator =
        RecordingGenerator::with_replies([GeneratorReply::Value(format!("{generated}\nignored"))]);
    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let _subscriber = tracing::subscriber::set_default(subscriber);
    let outcome = fixture
        .service
        .generate_chat_title(&fixture.threads[0].id, Some(generator.as_ref()))
        .await
        .unwrap();
    assert_eq!(outcome, ChatTitleOutcome::Generated);
    let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    for private in [owner, assistant, generated, "checkpoint-credential"] {
        assert!(!output.contains(private));
    }

    let request =
        ChatTitleGenerationRequest::new(None, owner, assistant, "checkpoint-credential".into());
    let title = ChatThreadTitle::new(generated.into()).unwrap();
    let debug = format!(
        "{request:?} {:?} {title:?}",
        ChatTitleGenerationError::unavailable()
    );
    for private in [owner, assistant, generated, "checkpoint-credential"] {
        assert!(!debug.contains(private));
    }
}

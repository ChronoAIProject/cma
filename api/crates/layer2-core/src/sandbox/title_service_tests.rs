use std::collections::VecDeque;
use std::future::pending;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime};

use crate::AgentId;
use tokio::sync::{Mutex, Semaphore};

use super::*;

const NOW: &str = "2026-09-05T00:00:00Z";

enum GeneratorReply {
    Value(String),
    Error,
    Pending,
    Wait(Arc<Semaphore>, String),
}

struct ObservedRequest {
    model: String,
    owner: String,
    assistant: String,
    bearer: String,
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

#[async_trait::async_trait]
impl AgentTitleGenerator for RecordingGenerator {
    async fn generate(
        &self,
        request: AgentTitleGenerationRequest,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.observed.lock().await.push(ObservedRequest {
            model: request.model().to_owned(),
            owner: request.owner_message().to_owned(),
            assistant: request.assistant_message().to_owned(),
            bearer: request.bearer().to_owned(),
        });
        match self.replies.lock().await.pop_front() {
            Some(GeneratorReply::Value(value)) => Ok(AgentTitleGenerationResponse::new(value)),
            Some(GeneratorReply::Error) | None => Err(AgentTitleGenerationError::unavailable()),
            Some(GeneratorReply::Pending) => pending().await,
            Some(GeneratorReply::Wait(gate, value)) => {
                let permit = gate.acquire().await.expect("the test gate remains open");
                permit.forget();
                Ok(AgentTitleGenerationResponse::new(value))
            }
        }
    }
}

struct Fixture {
    id: AgentId,
    store: Arc<InMemoryAgentTitleStore>,
    service: Arc<AgentTitleService>,
}

fn fixture(name: &str) -> Fixture {
    let id = AgentId(format!("agt_title_service_{name}"));
    let store = Arc::new(InMemoryAgentTitleStore::default());
    store.register_agent(id.clone()).unwrap();
    let service = AgentTitleService::new(Arc::clone(&store), format!("holder-{name}")).unwrap();
    Fixture { id, store, service }
}

fn append_turn(fixture: &Fixture, owner: &str, assistant: &str, sequence: u64) {
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: sequence,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: owner.into(),
        },
        AgentLogKind::AgentMessage {
            content: assistant.into(),
        },
        AgentLogKind::TurnCompleted {
            turn_sequence: sequence,
        },
    ] {
        fixture
            .store
            .append_agent_log(&fixture.id, NOW, kind)
            .unwrap();
    }
}

fn context() -> AgentTitleGenerationContext {
    AgentTitleGenerationContext::new("pinned-model", "private-bearer").unwrap()
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
async fn first_completed_turn_generates_once_from_bounded_private_inputs() {
    let fixture = fixture("first");
    let generator = RecordingGenerator::with_replies([GeneratorReply::Value(
        "  **\"Durable Agent Title\"**\nignored continuation  ".into(),
    )]);
    assert_eq!(
        fixture
            .service
            .generate_agent_title(&fixture.id, Some(&context()), Some(generator.as_ref()))
            .await
            .unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert_eq!(generator.calls(), 0);

    append_turn(
        &fixture,
        "How do durable titles work?",
        "They use a fenced write.",
        1,
    );
    assert_eq!(
        fixture
            .service
            .candidates(&PageRequest::default())
            .await
            .unwrap()
            .items,
        vec![fixture.id.clone()]
    );
    assert_eq!(
        fixture
            .service
            .generate_agent_title(&fixture.id, Some(&context()), Some(generator.as_ref()))
            .await
            .unwrap(),
        AgentTitleOutcome::Generated
    );
    let stored = fixture.service.title(&fixture.id).await.unwrap().unwrap();
    assert_eq!(stored.title.as_str(), "Durable Agent Title");
    assert_eq!(stored.source, AgentTitleSource::Generated);
    let observed = generator.observed.lock().await;
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].model, "pinned-model");
    assert_eq!(observed[0].owner, "How do durable titles work?");
    assert_eq!(observed[0].assistant, "They use a fenced write.");
    assert_eq!(observed[0].bearer, "private-bearer");
    drop(observed);

    append_turn(&fixture, "Later owner", "Later response", 2);
    assert_eq!(
        fixture
            .service
            .generate_agent_title(&fixture.id, Some(&context()), Some(generator.as_ref()))
            .await
            .unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert_eq!(generator.calls(), 1);
}

#[tokio::test]
async fn one_failure_or_invalid_value_retries_but_two_failures_fall_back() {
    let retry = fixture("retry");
    append_turn(&retry, "Explain retries", "They are bounded.", 1);
    let generator = RecordingGenerator::with_replies([
        GeneratorReply::Value("***".into()),
        GeneratorReply::Value("Retry Recovered Title".into()),
    ]);
    assert_eq!(
        retry
            .service
            .generate_agent_title(&retry.id, Some(&context()), Some(generator.as_ref()))
            .await
            .unwrap(),
        AgentTitleOutcome::Generated
    );
    assert_eq!(generator.calls(), 2);

    let failed = fixture("failed");
    let owner = format!("{}\nprivate owner continuation", "\u{754c}".repeat(80));
    append_turn(&failed, &owner, "The generator fails.", 1);
    let generator = RecordingGenerator::with_replies([
        GeneratorReply::Error,
        GeneratorReply::Error,
        GeneratorReply::Value("forbidden third attempt".into()),
    ]);
    assert_eq!(
        failed
            .service
            .generate_agent_title(&failed.id, Some(&context()), Some(generator.as_ref()))
            .await
            .unwrap(),
        AgentTitleOutcome::Fallback
    );
    assert_eq!(generator.calls(), 2);
    let stored = failed.service.title(&failed.id).await.unwrap().unwrap();
    assert_eq!(stored.source, AgentTitleSource::Fallback);
    assert_eq!(
        stored.title.as_str(),
        "\u{754c}".repeat(AGENT_TITLE_FALLBACK_SCALARS)
    );
}

#[tokio::test(start_paused = true)]
async fn each_of_two_attempts_has_an_exact_ten_second_timeout() {
    let fixture = fixture("timeout");
    append_turn(&fixture, "Timeout fallback", "No response arrives.", 1);
    let generator =
        RecordingGenerator::with_replies([GeneratorReply::Pending, GeneratorReply::Pending]);
    let service = Arc::clone(&fixture.service);
    let id = fixture.id.clone();
    let observed = Arc::clone(&generator);
    let task = tokio::spawn(async move {
        service
            .generate_agent_title(&id, Some(&context()), Some(observed.as_ref()))
            .await
    });

    wait_for_calls(&generator, 1).await;
    tokio::time::advance(Duration::from_secs(9)).await;
    tokio::task::yield_now().await;
    assert_eq!(generator.calls(), 1);
    assert!(!task.is_finished());
    tokio::time::advance(Duration::from_secs(1)).await;
    wait_for_calls(&generator, 2).await;
    tokio::time::advance(Duration::from_secs(10)).await;
    tokio::task::yield_now().await;
    assert_eq!(task.await.unwrap().unwrap(), AgentTitleOutcome::Fallback);
    assert_eq!(generator.calls(), 2);
}

#[tokio::test]
async fn concurrent_service_replicas_make_only_one_model_call() {
    let fixture = fixture("replicas");
    append_turn(&fixture, "Replica owner", "One worker may spend.", 1);
    let second = AgentTitleService::new(Arc::clone(&fixture.store), "holder-second").unwrap();
    let gate = Arc::new(Semaphore::new(0));
    let generator = RecordingGenerator::with_replies([GeneratorReply::Wait(
        Arc::clone(&gate),
        "Single Replica Winner".into(),
    )]);
    let first_service = Arc::clone(&fixture.service);
    let first_id = fixture.id.clone();
    let first_generator = Arc::clone(&generator);
    let first = tokio::spawn(async move {
        first_service
            .generate_agent_title(&first_id, Some(&context()), Some(first_generator.as_ref()))
            .await
    });
    wait_for_calls(&generator, 1).await;
    assert_eq!(
        second
            .generate_agent_title(&fixture.id, Some(&context()), Some(generator.as_ref()))
            .await
            .unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert_eq!(generator.calls(), 1);
    gate.add_permits(1);
    assert_eq!(first.await.unwrap().unwrap(), AgentTitleOutcome::Generated);
    assert_eq!(generator.calls(), 1);
}

#[tokio::test]
async fn expired_takeover_uses_fallback_without_a_second_model_call() {
    let fixture = fixture("takeover");
    append_turn(&fixture, "Crash recovery owner", "Use fallback.", 1);
    let stale = fixture
        .store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(fixture.id.clone(), "crashed-holder"),
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stale.mode(), AgentTitleClaimMode::Generate);
    let generator =
        RecordingGenerator::with_replies([GeneratorReply::Value("must never be called".into())]);
    assert_eq!(
        fixture
            .service
            .generate_agent_title(&fixture.id, Some(&context()), Some(generator.as_ref()))
            .await
            .unwrap(),
        AgentTitleOutcome::Fallback
    );
    assert_eq!(generator.calls(), 0);
    assert_eq!(
        fixture
            .service
            .title(&fixture.id)
            .await
            .unwrap()
            .unwrap()
            .title
            .as_str(),
        "Crash recovery owner"
    );
    assert!(!fixture
        .store
        .complete_agent_title(
            &stale,
            &AgentTitle::new("Stale Result").unwrap(),
            AgentTitleSource::Generated,
            NOW.into(),
        )
        .await
        .unwrap());
}

#[tokio::test]
async fn user_title_and_missing_dependencies_suppress_model_work() {
    let user = fixture("user");
    let title = AgentTitle::new("Chosen By User").unwrap();
    assert_eq!(
        user.service.set_user_title(&user.id, &title).await.unwrap(),
        SetAgentTitleOutcome::Set
    );
    assert_eq!(
        user.service.set_user_title(&user.id, &title).await.unwrap(),
        SetAgentTitleOutcome::AlreadySet
    );
    append_turn(&user, "Owner", "Response", 1);
    let unused = RecordingGenerator::with_replies([GeneratorReply::Value("unused".into())]);
    assert_eq!(
        user.service
            .generate_agent_title(&user.id, Some(&context()), Some(unused.as_ref()))
            .await
            .unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert_eq!(unused.calls(), 0);

    for (name, with_context, with_generator) in
        [("no_context", false, true), ("no_generator", true, false)]
    {
        let fixture = fixture(name);
        append_turn(&fixture, "Dependency fallback", "Response", 1);
        let generator = RecordingGenerator::with_replies([GeneratorReply::Value("unused".into())]);
        let context = context();
        let outcome = fixture
            .service
            .generate_agent_title(
                &fixture.id,
                with_context.then_some(&context),
                with_generator.then_some(generator.as_ref()),
            )
            .await
            .unwrap();
        assert_eq!(outcome, AgentTitleOutcome::Fallback);
        assert_eq!(generator.calls(), 0);
    }
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
async fn private_content_never_enters_service_tracing() {
    let fixture = fixture("privacy");
    let owner = "owner-private-service-marker";
    let assistant = "assistant-private-service-marker";
    let generated = "generated-private-service-marker";
    append_turn(&fixture, owner, assistant, 1);
    let generator = RecordingGenerator::with_replies([GeneratorReply::Value(generated.into())]);
    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(logs.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    assert_eq!(
        fixture
            .service
            .generate_agent_title(&fixture.id, Some(&context()), Some(generator.as_ref()))
            .await
            .unwrap(),
        AgentTitleOutcome::Generated
    );
    let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    for private in [owner, assistant, generated, "private-bearer"] {
        assert!(!output.contains(private));
    }
}

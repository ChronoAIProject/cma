use std::io;

use layer2_core::sandbox::{
    AgentTranscriptPage, AgentTranscriptPageRequest, AgentTranscriptRole, AgentTurnOutcome,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use super::Problem;

const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;

#[derive(Serialize, ToSchema)]
pub(super) struct TranscriptPackage<'a> {
    owner_subject: &'a str,
    pub(super) manifest: Manifest,
    payload: String,
}

#[derive(Serialize, ToSchema)]
pub(super) struct Manifest {
    format: &'static str,
    version: u8,
    agent_id: String,
    through_seq: u64,
    window: RequestedWindow,
    coverage: Coverage,
    records: usize,
    pub(super) size_bytes: usize,
    sha256: String,
    next: Option<Continuation>,
}

#[derive(Serialize, ToSchema)]
struct Coverage {
    raw_history: bool,
    completeness: &'static str,
    retention_window: &'static str,
    gaps: &'static str,
    includes: [&'static str; 3],
    excludes: [&'static str; 4],
}

#[derive(Serialize, ToSchema)]
struct Continuation {
    agent: String,
    turn: u64,
    after: u64,
    through: u64,
    limit: u16,
}

#[derive(Serialize, ToSchema)]
struct RequestedWindow {
    #[schema(required = true)]
    turn: Option<u64>,
    #[schema(required = true)]
    after: Option<u64>,
    #[schema(required = true)]
    through: Option<u64>,
    limit: u16,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Record<'a> {
    Turn {
        turn_sequence: u64,
        started_at: &'a str,
        outcome: &'static str,
        finished_at: Option<&'a str>,
        additional_output_retained: bool,
        known_output_gap: bool,
    },
    Message {
        turn_sequence: u64,
        sequence: u64,
        role: &'static str,
        content: &'a str,
        appended_at: &'a str,
    },
}

pub(super) fn transcript_package<'a>(
    owner_subject: &'a str,
    request: &AgentTranscriptPageRequest,
    page: &AgentTranscriptPage,
) -> Result<TranscriptPackage<'a>, Problem> {
    let transcript = &page.transcript;
    let mut buffer = Payload::default();
    for turn in &transcript.turns {
        let (outcome, finished_at) = match &turn.outcome {
            AgentTurnOutcome::Active => ("active", None),
            AgentTurnOutcome::Completed { at } => ("completed", Some(at.as_str())),
            AgentTurnOutcome::Failed { at, .. } => ("failed", Some(at.as_str())),
            AgentTurnOutcome::Interrupted { at } => ("interrupted", Some(at.as_str())),
        };
        buffer.record(Record::Turn {
            turn_sequence: turn.turn_sequence,
            started_at: &turn.started_at,
            outcome,
            finished_at,
            additional_output_retained: turn.retained_output.is_some_and(|value| value.available),
            known_output_gap: matches!(
                &turn.outcome,
                AgentTurnOutcome::Failed { failure, .. } if failure.class == "conversation_output_gap"
            ),
        })?;
        for message in &turn.messages {
            let role = match message.role {
                AgentTranscriptRole::Owner => "user",
                AgentTranscriptRole::Agent => "assistant",
            };
            buffer.record(Record::Message {
                turn_sequence: turn.turn_sequence,
                sequence: message.sequence,
                role,
                content: &message.content,
                appended_at: &message.appended_at,
            })?;
        }
    }
    let next = page
        .next
        .as_ref()
        .map(|next| continuation(transcript.agent_id.as_str(), next))
        .transpose()?;
    Ok(TranscriptPackage {
        owner_subject,
        manifest: Manifest {
            format: "cma.retained-conversation-page",
            version: 1,
            agent_id: transcript.agent_id.to_string(),
            through_seq: transcript.through_seq,
            window: RequestedWindow {
                turn: request.turn,
                after: request.after,
                through: request.through,
                limit: request.limit,
            },
            coverage: Coverage {
                raw_history: false,
                completeness: "not_certified",
                retention_window: "not_reported_by_source",
                gaps: "known_output_gaps_are_marked",
                includes: [
                    "recorded_user_messages",
                    "recorded_assistant_messages",
                    "turn_outcomes",
                ],
                excludes: [
                    "raw_runtime_records",
                    "tool_records",
                    "unrecorded_output",
                    "private_metadata",
                ],
            },
            records: buffer.records,
            size_bytes: buffer.bytes.len(),
            sha256: format!("{:x}", Sha256::digest(&buffer.bytes)),
            next,
        },
        payload: String::from_utf8(buffer.bytes).map_err(|_| Problem::Unavailable)?,
    })
}

fn continuation(agent: &str, next: &AgentTranscriptPageRequest) -> Result<Continuation, Problem> {
    let (Some(turn), Some(after), Some(through)) = (next.turn, next.after, next.through) else {
        return Err(Problem::Unavailable);
    };
    next.validate().map_err(|_| Problem::Unavailable)?;
    Ok(Continuation {
        agent: agent.to_owned(),
        turn,
        after,
        through,
        limit: next.limit,
    })
}

#[derive(Default)]
struct Payload {
    records: usize,
    bytes: Vec<u8>,
}

impl Payload {
    fn record(&mut self, value: Record<'_>) -> Result<(), Problem> {
        use std::io::Write as _;
        serde_json::to_writer(&mut *self, &value).map_err(|error| {
            if error.is_io() {
                Problem::TooLarge
            } else {
                Problem::Unavailable
            }
        })?;
        self.write_all(b"\n").map_err(|_| Problem::TooLarge)?;
        self.records += 1;
        Ok(())
    }
}

impl io::Write for Payload {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_PAYLOAD_BYTES - self.bytes.len() {
            return Err(io::Error::other(
                "privacy export payload exceeds delivery limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

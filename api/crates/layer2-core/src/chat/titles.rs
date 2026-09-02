//! Fenced, write-once names for durable chat threads.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::service::{store_failed, SessionService};
use crate::store::{ChatThreadTitle, ChatTitleClaimMode, ChatTitleClaimRequest, Page, PageRequest};
use crate::{CoreError, SessionId};

pub const CHAT_TITLE_SYSTEM_PROMPT: &str =
    "Create a 3-6 word conversation title. Return only the title, with no quotes or ornament.";
pub const CHAT_TITLE_INPUT_BYTES: usize = 2 * 1024;
pub const CHAT_TITLE_MAX_CHARACTERS: usize = 64;
pub const CHAT_TITLE_FALLBACK_CHARACTERS: usize = 48;
pub const CHAT_TITLE_CALL_TIMEOUT_SECONDS: u64 = 10;
const CHAT_TITLE_CLAIM_SECONDS: u64 = 45;
const CHAT_TITLE_ATTEMPTS: usize = 2;
const EMPTY_FALLBACK_TITLE: &str = "New conversation";

/// Private inputs for one model call.
///
/// Debug output carries only sizes. Transcript and credential content are
/// intentionally unavailable to application logging through this type.
#[derive(Clone)]
pub struct ChatTitleGenerationRequest {
    model: Option<String>,
    owner_message: String,
    assistant_message: String,
    bearer: String,
}

impl ChatTitleGenerationRequest {
    /// Construct one adapter request from already-authorized private inputs.
    #[doc(hidden)]
    pub fn new(
        model: Option<String>,
        owner_message: &str,
        assistant_message: &str,
        bearer: String,
    ) -> Self {
        Self {
            model,
            owner_message: truncate_utf8_bytes(owner_message, CHAT_TITLE_INPUT_BYTES),
            assistant_message: truncate_utf8_bytes(assistant_message, CHAT_TITLE_INPUT_BYTES),
            bearer,
        }
    }

    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    pub fn owner_message(&self) -> &str {
        &self.owner_message
    }

    pub fn assistant_message(&self) -> &str {
        &self.assistant_message
    }

    pub fn bearer(&self) -> &str {
        &self.bearer
    }
}

impl std::fmt::Debug for ChatTitleGenerationRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChatTitleGenerationRequest")
            .field("model", &self.model)
            .field(
                "owner_message",
                &format_args!("<redacted {} bytes>", self.owner_message.len()),
            )
            .field(
                "assistant_message",
                &format_args!("<redacted {} bytes>", self.assistant_message.len()),
            )
            .field(
                "bearer",
                &format_args!("<redacted {} bytes>", self.bearer.len()),
            )
            .finish()
    }
}

/// Privacy-safe failure from the deployment model adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("chat title generation failed")]
pub struct ChatTitleGenerationError;

impl ChatTitleGenerationError {
    pub fn unavailable() -> Self {
        Self
    }
}

/// Deployment-specific model call used only after a durable claim is held.
#[async_trait]
pub trait ChatTitleGenerator: Send + Sync {
    async fn generate(
        &self,
        request: ChatTitleGenerationRequest,
    ) -> Result<String, ChatTitleGenerationError>;
}

/// Content-free result of settling one title candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatTitleOutcome {
    Generated,
    Fallback,
    Skipped,
}

impl SessionService {
    /// Read one bounded candidate page through the session store.
    pub async fn chat_title_candidates(
        &self,
        page: PageRequest,
    ) -> Result<Page<SessionId>, CoreError> {
        self.store
            .chat_title_candidates(page)
            .await
            .map_err(store_failed)
    }

    /// Claim and settle one titleless thread after its first completed turn.
    pub async fn generate_chat_title(
        self: &Arc<Self>,
        id: &SessionId,
        generator: Option<&dyn ChatTitleGenerator>,
    ) -> Result<ChatTitleOutcome, CoreError> {
        let request = ChatTitleClaimRequest::new(
            id.clone(),
            &self.coordination_holder,
            Duration::from_secs(CHAT_TITLE_CLAIM_SECONDS),
        );
        let Some(claim) = self
            .store
            .acquire_chat_title_claim(&request)
            .await
            .map_err(store_failed)?
        else {
            return Ok(ChatTitleOutcome::Skipped);
        };

        let transcript = self
            .store
            .chat_title_transcript(id)
            .await
            .map_err(store_failed)?;
        if !transcript.turn_completed {
            return Ok(ChatTitleOutcome::Skipped);
        }
        let fallback = fallback_title(transcript.owner_message.as_deref());

        let generated = if claim.mode() == ChatTitleClaimMode::Generate {
            match (
                generator,
                transcript.owner_message.as_deref(),
                transcript.assistant_message.as_deref(),
            ) {
                (Some(generator), Some(owner), Some(assistant)) => {
                    self.generate_claimed_title(id, generator, owner, assistant)
                        .await
                }
                _ => None,
            }
        } else {
            None
        };
        let outcome = if generated.is_some() {
            ChatTitleOutcome::Generated
        } else {
            ChatTitleOutcome::Fallback
        };
        let title = generated.unwrap_or(fallback);

        if !self
            .store
            .complete_chat_title(&claim, &title)
            .await
            .map_err(store_failed)?
        {
            return Ok(ChatTitleOutcome::Skipped);
        }
        self.refresh_live_title(id).await;
        Ok(outcome)
    }

    async fn generate_claimed_title(
        &self,
        id: &SessionId,
        generator: &dyn ChatTitleGenerator,
        owner_message: &str,
        assistant_message: &str,
    ) -> Option<ChatThreadTitle> {
        let record = self.store.get(id).await.ok().flatten()?;
        let origin = record.chat_origin.as_ref()?;
        if origin.title.is_some() {
            return None;
        }
        let hosts = self.chat_hosts.as_ref()?;
        let host = hosts.get(&origin.host_id).await.ok().flatten()?;
        if host.owner != record.session.owner
            || !host.threads.iter().any(|thread| {
                thread.session_id == *id
                    && thread.worktree_name == origin.worktree_name
                    && thread.worktree_path == origin.worktree_path
                    && !thread.removed
            })
        {
            return None;
        }

        let services = self.services_for_record(&record).await;
        let bearer = self
            .credential_for(id, &record.session.owner, &services, None)
            .await
            .ok()
            .flatten()?;
        let request = ChatTitleGenerationRequest::new(
            host.pinned_settings.settings.agent.model.clone(),
            owner_message,
            assistant_message,
            bearer,
        );

        for _ in 0..CHAT_TITLE_ATTEMPTS {
            let generated = tokio::time::timeout(
                Duration::from_secs(CHAT_TITLE_CALL_TIMEOUT_SECONDS),
                generator.generate(request.clone()),
            )
            .await;
            if let Ok(Ok(value)) = generated {
                if let Some(title) = sanitize_generated_title(&value) {
                    return Some(title);
                }
            }
        }
        None
    }

    async fn refresh_live_title(&self, id: &SessionId) {
        let Ok(Some(durable)) = self.store.get(id).await else {
            return;
        };
        let mut entries = self.entries.write().await;
        let Some(entry) = entries.get_mut(id) else {
            return;
        };
        if entry
            .chat_origin
            .as_ref()
            .zip(durable.chat_origin.as_ref())
            .is_none_or(|(live, stored)| live.host_id != stored.host_id)
        {
            return;
        }
        entry.chat_origin.clone_from(&durable.chat_origin);
        entry.session.chat.clone_from(&durable.session.chat);
        entry.event_log.observe_record(&durable);
    }
}

fn sanitize_generated_title(value: &str) -> Option<ChatThreadTitle> {
    let line = safe_first_line(value);
    let bounded: String = line.chars().take(CHAT_TITLE_MAX_CHARACTERS).collect();
    ChatThreadTitle::new(bounded.trim().to_owned())
}

fn fallback_title(owner_message: Option<&str>) -> ChatThreadTitle {
    let candidate: String = owner_message
        .map(safe_first_line)
        .unwrap_or_default()
        .chars()
        .take(CHAT_TITLE_FALLBACK_CHARACTERS)
        .collect();
    ChatThreadTitle::new(candidate.trim().to_owned())
        .unwrap_or_else(|| ChatThreadTitle::new(EMPTY_FALLBACK_TITLE.into()).unwrap())
}

fn safe_first_line(value: &str) -> String {
    value
        .split(['\r', '\n'])
        .next()
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .trim()
        .to_owned()
}

fn truncate_utf8_bytes(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_inputs_are_utf8_safe_and_debug_redacted() {
        let marker = "private-title-input-marker";
        let credential = "private-title-credential-marker";
        let request = ChatTitleGenerationRequest::new(
            Some("pinned-model".into()),
            &format!("{}{}", "x".repeat(CHAT_TITLE_INPUT_BYTES - 1), "\u{754c}"),
            marker,
            credential.into(),
        );
        assert!(request.owner_message().len() <= CHAT_TITLE_INPUT_BYTES);
        assert_eq!(request.assistant_message(), marker);
        let debug = format!("{request:?}");
        assert!(!debug.contains(marker));
        assert!(!debug.contains(credential));
        assert!(debug.contains("pinned-model"));
    }

    #[test]
    fn generated_titles_are_single_line_control_free_trimmed_and_bounded() {
        let title = sanitize_generated_title(&format!(
            "  Safe\tTitle{}\nignored title-content marker  ",
            "x".repeat(100)
        ))
        .expect("a non-empty first line remains");
        assert_eq!(title.as_str().chars().count(), CHAT_TITLE_MAX_CHARACTERS);
        assert!(!title.as_str().chars().any(char::is_control));
        assert!(!title.as_str().contains("ignored"));
        assert_eq!(title.as_str(), title.as_str().trim());
        assert!(sanitize_generated_title("\r\n\t\0").is_none());
    }

    #[test]
    fn fallback_uses_only_the_first_owner_line_and_exact_character_bound() {
        let title = fallback_title(Some(&format!(
            "  {}\nprivate second line  ",
            "\u{754c}".repeat(80)
        )));
        assert_eq!(
            title.as_str().chars().count(),
            CHAT_TITLE_FALLBACK_CHARACTERS
        );
        assert!(!title.as_str().contains("private"));
        assert_eq!(
            fallback_title(Some("\nsecret")).as_str(),
            EMPTY_FALLBACK_TITLE
        );
        assert_eq!(fallback_title(None).as_str(), EMPTY_FALLBACK_TITLE);
    }
}

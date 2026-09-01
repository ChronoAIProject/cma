//! Durable references to one coherent chat-host recovery point.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::SessionId;

pub const MAX_CHAT_CONVERSATION_SNAPSHOT_BYTES: u64 = 256 * 1024 * 1024;

/// One exact conversation object belonging to a host checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatCheckpointConversationRef {
    pub session_id: SessionId,
    pub object_reference: String,
    pub sha256: String,
    pub conversation_id: String,
}

/// Captured bytes and identity used while sealing and verifying one thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatCheckpointConversation {
    pub session_id: SessionId,
    pub object_reference: String,
    pub conversation_id: String,
    pub snapshot: Vec<u8>,
}

/// Immutable conversation and workspace objects sealed as one recovery point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatCheckpointRef {
    pub workspace_object_reference: String,
    pub workspace_archive_sha256: String,
    pub workspace_manifest_digest: String,
    pub conversations: Vec<ChatCheckpointConversationRef>,
    pub captured_at: String,
    pub checkpoint_version: u64,
    pub pair_sha256: String,
}

impl ChatCheckpointRef {
    /// Seal one complete workspace plus every live conversation as a set.
    pub fn seal(
        workspace_object_reference: impl Into<String>,
        workspace: &layer3_adapter::WorkspaceCheckpointPackage,
        mut conversations: Vec<ChatCheckpointConversation>,
        captured_at: impl Into<String>,
    ) -> Result<Self, String> {
        conversations.sort_by(|left, right| {
            left.session_id
                .as_str()
                .cmp(right.session_id.as_str())
                .then_with(|| left.conversation_id.cmp(&right.conversation_id))
        });
        let conversations = conversations
            .into_iter()
            .map(|conversation| {
                validate_snapshot_bytes(&conversation.snapshot)?;
                Ok(ChatCheckpointConversationRef {
                    session_id: conversation.session_id,
                    object_reference: conversation.object_reference,
                    sha256: digest(&conversation.snapshot),
                    conversation_id: conversation.conversation_id,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut reference = Self {
            workspace_object_reference: workspace_object_reference.into(),
            workspace_archive_sha256: workspace.archive_sha256.clone(),
            workspace_manifest_digest: workspace.manifest_digest.clone(),
            conversations,
            captured_at: captured_at.into(),
            checkpoint_version: workspace.version,
            pair_sha256: String::new(),
        };
        reference.pair_sha256 = reference.canonical_digest()?;
        reference.validate()?;
        Ok(reference)
    }

    /// Validate the durable identity without trusting any referenced object.
    pub fn validate(&self) -> Result<(), String> {
        if self.workspace_object_reference.trim().is_empty() {
            return Err(field("workspace_object_reference", "is empty"));
        }
        require_sha256("workspace_archive_sha256", &self.workspace_archive_sha256)?;
        require_sha256("workspace_manifest_digest", &self.workspace_manifest_digest)?;
        require_sha256("pair_sha256", &self.pair_sha256)?;
        if self.checkpoint_version == 0 {
            return Err(field("checkpoint_version", "must be positive"));
        }
        if crate::seconds_from_rfc3339(&self.captured_at).is_none() {
            return Err(field("captured_at", "is not a CMA timestamp"));
        }
        if self.conversations.is_empty() {
            return Err(field(
                "conversations",
                "a workspace-only capture is not a recovery point",
            ));
        }

        let mut sessions = BTreeSet::new();
        let mut conversation_ids = BTreeSet::new();
        let mut object_references = BTreeSet::from([self.workspace_object_reference.as_str()]);
        let mut previous_session: Option<&str> = None;
        for conversation in &self.conversations {
            if conversation.session_id.as_str().trim().is_empty()
                || conversation.conversation_id.trim().is_empty()
            {
                return Err(field(
                    "conversations.identity",
                    "must name a session and durable conversation",
                ));
            }
            if previous_session.is_some_and(|previous| previous >= conversation.session_id.as_str())
            {
                return Err(field(
                    "conversations",
                    "must be in canonical session-id order",
                ));
            }
            previous_session = Some(conversation.session_id.as_str());
            if !sessions.insert(conversation.session_id.as_str()) {
                return Err(field("conversations.session_id", "is duplicated"));
            }
            if !conversation_ids.insert(conversation.conversation_id.as_str()) {
                return Err(field("conversations.conversation_id", "is duplicated"));
            }
            if conversation.object_reference.trim().is_empty()
                || !object_references.insert(conversation.object_reference.as_str())
            {
                return Err(field(
                    "conversations.object_reference",
                    "must name a distinct object",
                ));
            }
            require_sha256("conversations.sha256", &conversation.sha256)?;
        }
        if self.canonical_digest()? != self.pair_sha256 {
            return Err(field(
                "pair_sha256",
                "does not bind the exact workspace and conversation set",
            ));
        }
        Ok(())
    }

    /// Verify captured or downloaded bytes against every sealed digest.
    pub fn validate_bytes(
        &self,
        workspace: &layer3_adapter::WorkspaceCheckpointPackage,
        conversations: &[ChatCheckpointConversation],
    ) -> Result<(), String> {
        self.validate()?;
        if workspace.version != self.checkpoint_version
            || workspace.manifest_digest != self.workspace_manifest_digest
            || workspace.archive_sha256 != self.workspace_archive_sha256
            || digest(&workspace.archive) != self.workspace_archive_sha256
        {
            return Err(field(
                "workspace",
                "bytes do not match the sealed workspace identity",
            ));
        }
        if conversations.len() != self.conversations.len() {
            return Err(field(
                "conversations",
                "bytes do not cover the sealed conversation set",
            ));
        }
        for (expected, observed) in self.conversations.iter().zip(conversations) {
            validate_snapshot_bytes(&observed.snapshot)?;
            if expected.session_id != observed.session_id
                || expected.object_reference != observed.object_reference
                || expected.conversation_id != observed.conversation_id
                || expected.sha256 != digest(&observed.snapshot)
            {
                return Err(field(
                    "conversations",
                    "bytes do not match one sealed conversation identity",
                ));
            }
        }
        Ok(())
    }

    fn canonical_digest(&self) -> Result<String, String> {
        let mut digest = Sha256::new();
        digest.update(b"cma-chat-checkpoint-v1\0");
        digest.update(self.checkpoint_version.to_be_bytes());
        hash_text(&mut digest, &self.workspace_object_reference)?;
        hash_text(&mut digest, &self.workspace_archive_sha256)?;
        hash_text(&mut digest, &self.workspace_manifest_digest)?;
        digest.update(
            u64::try_from(self.conversations.len())
                .map_err(|_| field("conversations", "exceed the hash wire bound"))?
                .to_be_bytes(),
        );
        for conversation in &self.conversations {
            hash_text(&mut digest, conversation.session_id.as_str())?;
            hash_text(&mut digest, &conversation.object_reference)?;
            hash_text(&mut digest, &conversation.sha256)?;
            hash_text(&mut digest, &conversation.conversation_id)?;
        }
        hash_text(&mut digest, &self.captured_at)?;
        Ok(hex(&digest.finalize()))
    }

    pub(crate) fn reseal(&mut self) -> Result<(), String> {
        self.pair_sha256 = self.canonical_digest()?;
        self.validate()
    }
}

fn validate_snapshot_bytes(snapshot: &[u8]) -> Result<(), String> {
    let bytes = u64::try_from(snapshot.len()).unwrap_or(u64::MAX);
    if bytes == 0 || bytes > MAX_CHAT_CONVERSATION_SNAPSHOT_BYTES {
        return Err(field(
            "conversations.snapshot",
            format!("must be between 1 and {MAX_CHAT_CONVERSATION_SNAPSHOT_BYTES} bytes"),
        ));
    }
    Ok(())
}

fn require_sha256(name: &str, value: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(field(name, "must be a SHA-256 digest"));
    }
    Ok(())
}

fn hash_text(digest: &mut Sha256, value: &str) -> Result<(), String> {
    digest.update(
        u64::try_from(value.len())
            .map_err(|_| field("identity", "contains a value outside the hash wire bound"))?
            .to_be_bytes(),
    );
    digest.update(value.as_bytes());
    Ok(())
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    const CHARS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(CHARS[usize::from(byte >> 4)]));
        out.push(char::from(CHARS[usize::from(byte & 0x0f)]));
    }
    out
}

fn field(name: &str, detail: impl AsRef<str>) -> String {
    format!("chat checkpoint {name} {}", detail.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer3_adapter::{WorkspaceCheckpointPackage, WorkspaceWriterState};

    fn workspace() -> WorkspaceCheckpointPackage {
        let archive = b"complete git workspace".to_vec();
        WorkspaceCheckpointPackage {
            epoch: 3,
            version: 7,
            source_revision: "a".repeat(40),
            manifest_digest: "b".repeat(64),
            archive_sha256: digest(&archive),
            entries: Vec::new(),
            total_bytes: 0,
            quiesced_writers: 2,
            writer_state: WorkspaceWriterState::HeldForTeardown,
            writer_resume_token: Some("hold-7".into()),
            includes_git_metadata: true,
            archive,
        }
    }

    fn conversation(session: &str, conversation: &str) -> ChatCheckpointConversation {
        ChatCheckpointConversation {
            session_id: SessionId(session.into()),
            object_reference: format!("sessions/{session}/handover.snapshot"),
            conversation_id: conversation.into(),
            snapshot: format!("snapshot for {conversation}").into_bytes(),
        }
    }

    fn sealed() -> (
        ChatCheckpointRef,
        WorkspaceCheckpointPackage,
        Vec<ChatCheckpointConversation>,
    ) {
        let workspace = workspace();
        let conversations = vec![
            conversation("sesn_b", "conv-b"),
            conversation("sesn_a", "conv-a"),
        ];
        let reference = ChatCheckpointRef::seal(
            "chat-hosts/chho_1/checkpoint-v1",
            &workspace,
            conversations.clone(),
            "2026-09-01T00:00:00Z",
        )
        .unwrap();
        let mut canonical = conversations;
        canonical.sort_by(|left, right| left.session_id.as_str().cmp(right.session_id.as_str()));
        (reference, workspace, canonical)
    }

    #[test]
    fn a_complete_set_is_canonical_sealed_and_byte_verifiable() {
        let (reference, workspace, conversations) = sealed();
        assert_eq!(reference.conversations[0].session_id.as_str(), "sesn_a");
        reference.validate().unwrap();
        reference
            .validate_bytes(&workspace, &conversations)
            .unwrap();
    }

    #[test]
    fn a_workspace_without_any_conversation_is_not_a_recovery_point() {
        let error = ChatCheckpointRef::seal(
            "chat-hosts/chho_1/checkpoint-v1",
            &workspace(),
            Vec::new(),
            "2026-09-01T00:00:00Z",
        )
        .unwrap_err();
        assert!(error.contains("workspace-only"), "got {error}");
    }

    #[test]
    fn duplicate_session_conversation_and_object_identities_are_refused() {
        let duplicate_cases = [
            vec![
                conversation("sesn_a", "conv-a"),
                conversation("sesn_a", "conv-b"),
            ],
            vec![
                conversation("sesn_a", "conv-a"),
                conversation("sesn_b", "conv-a"),
            ],
            vec![
                conversation("sesn_a", "conv-a"),
                conversation("sesn_b", "conv-b"),
            ],
        ];
        for (index, mut conversations) in duplicate_cases.into_iter().enumerate() {
            if index == 2 {
                conversations[1].object_reference = conversations[0].object_reference.clone();
            }
            assert!(ChatCheckpointRef::seal(
                "chat-hosts/chho_1/checkpoint-v1",
                &workspace(),
                conversations,
                "2026-09-01T00:00:00Z",
            )
            .is_err());
        }
    }

    #[test]
    fn changing_any_metadata_after_sealing_breaks_the_pair_digest() {
        let (reference, _, _) = sealed();
        let mut changed = reference.clone();
        changed.checkpoint_version += 1;
        assert!(changed.validate().is_err());

        let mut changed = reference.clone();
        changed.workspace_manifest_digest = "c".repeat(64);
        assert!(changed.validate().is_err());

        let mut changed = reference;
        changed.conversations[0].conversation_id.push_str("-other");
        assert!(changed.validate().is_err());
    }

    #[test]
    fn tampered_workspace_or_conversation_bytes_fail_closed() {
        let (reference, workspace, conversations) = sealed();
        let mut corrupt_workspace = workspace.clone();
        corrupt_workspace.archive.push(0);
        assert!(reference
            .validate_bytes(&corrupt_workspace, &conversations)
            .is_err());

        let mut corrupt_conversations = conversations;
        corrupt_conversations[0].snapshot.push(0);
        assert!(reference
            .validate_bytes(&workspace, &corrupt_conversations)
            .is_err());
    }
}

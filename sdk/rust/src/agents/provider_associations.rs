//! Retained provider membership is independent of legacy channel-usage history.
use super::Agents;
use crate::{Error, PageOptions, Result, resources::identity};
use serde::Deserialize;

string_enum! { /// Whether the provider owns or borrows the retained Agent association.
    pub enum ProviderAssociationRelationship { Managed => "managed", Borrowed => "borrowed" }
}
string_enum! { /// Retained conversation lifecycle state; not permission to dispatch.
    pub enum ProviderAssociationConversationState {
        Provisioning => "provisioning", Ready => "ready", Failed => "failed", Retiring => "retiring", Retired => "retired"
    }
}
string_enum! { /// Current parent binding state, also returned for old generations.
    pub enum ProviderAssociationBindingState {
        Provisioning => "provisioning", Ready => "ready", Failed => "failed", Retired => "retired"
    }
}
/// An authoritative Manager-owned association; no inferred Gateway channel identity.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProviderAssociation {
    /// Retained parent binding identifier.
    pub binding_id: String,
    /// Generation when this association was reserved.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub binding_generation: u64,
    /// Provider's generation-scoped conversation identifier.
    pub provider_conversation_id: String,
    /// Managed or borrowed, independent of Agent creation origin.
    pub relationship: ProviderAssociationRelationship,
    /// Exact retained Sandbox identifier.
    pub sandbox_id: String,
    /// Exact retained Workspace identifier.
    pub workspace_id: String,
    /// Exact retained Worktree identifier.
    pub worktree_id: String,
    /// Conversation's lifecycle, including retirement.
    pub conversation_state: ProviderAssociationConversationState,
    /// Current parent lifecycle, separate from conversation lifecycle.
    pub binding_state: ProviderAssociationBindingState,
    /// Current parent generation.
    #[serde(deserialize_with = "crate::resources::deserialize_positive")]
    pub current_binding_generation: u64,
    /// Whether this association belongs to the parent's current generation.
    pub is_current_generation: bool,
    /// Original reservation creation time, not first event use.
    pub association_created_at: String,
    /// Last reservation update time, not last event use.
    pub association_updated_at: String,
}
/// Bounded membership page requiring current Agent management authority.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentProviderAssociations {
    /// Exact requested Agent identifier.
    pub agent_id: String,
    /// Resource owner's subject, not the provider manager's subject.
    pub owner: String,
    /// Exact Agent incarnation creation timestamp.
    pub agent_created_at: String,
    /// Retained associations ordered by binding, generation and conversation.
    pub items: Vec<ProviderAssociation>,
    /// Opaque continuation scoped to this owner and Agent incarnation.
    pub next: Option<String>,
}
fn cursor(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
impl Agents<'_> {
    /// Read retained memberships, including retired generations. This performs
    /// no lifecycle operation and does not imply safe deletion or event usage.
    pub async fn provider_associations(
        &self,
        agent_id: &str,
        options: &PageOptions,
    ) -> Result<AgentProviderAssociations> {
        if options.limit.is_some_and(|n| n == 0 || n > 200)
            || options.cursor.as_ref().is_some_and(|c| !cursor(c))
        {
            return Err(Error::validation(
                "provider associations require limit 1 through 200 and an opaque continuation",
            ));
        }
        let value: AgentProviderAssociations = self
            .client
            .get(
                &["agents", agent_id, "provider-associations"],
                &options.query(),
            )
            .await?;
        identity(&value.agent_id, agent_id)?;
        let limit = usize::from(options.limit.unwrap_or(50));
        if value.items.len() > limit {
            return Err(Error::protocol(
                "provider association page exceeded its limit",
            ));
        }
        let mut previous = String::new();
        for item in &value.items {
            let key = format!(
                "{}:{:020}:{}",
                item.binding_id, item.binding_generation, item.provider_conversation_id
            );
            if key <= previous
                || item.binding_generation > item.current_binding_generation
                || item.is_current_generation
                    != (item.binding_generation == item.current_binding_generation)
            {
                return Err(Error::protocol(
                    "provider association identity or generation is inconsistent",
                ));
            }
            previous = key;
        }
        if value.next.as_ref().is_some_and(|next| {
            !cursor(next) || Some(next) == options.cursor.as_ref() || value.items.len() != limit
        }) {
            return Err(Error::protocol(
                "provider association continuation did not advance a full page",
            ));
        }
        Ok(value)
    }
}

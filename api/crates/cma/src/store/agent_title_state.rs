//! Validated Mongo projection for durable agent-title coordination.

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{
    Agent, AgentTitle, AgentTitleClaim, AgentTitleClaimMode, AgentTitleClaimRequest,
    AgentTitleSource, AgentTitleStoreError, StoredAgentTitle, AGENT_TITLE_CLAIM_SECONDS,
};
use layer2_core::seconds_from_rfc3339;
use mongodb::bson::{doc, Bson, DateTime, Document};

#[path = "agent_title_claim_data.rs"]
mod claim_data;
pub(super) use claim_data::checked_claim_times;

pub(super) const TITLE: &str = "title";
const SOURCE: &str = "title_source";
const SET_AT: &str = "title_set_at";
const ATTEMPTED: &str = "title_attempted";
const LEASE_HOLDER: &str = "title_lease_holder";
const LEASE_TOKEN: &str = "title_lease_token";
const LEASE_MODE: &str = "title_lease_mode";
const LEASE_ACQUIRED_AT: &str = "title_lease_acquired_at";
const LEASE_EXPIRES_AT: &str = "title_lease_expires_at";
const COMMIT_HOLDER: &str = "title_commit_holder";
const COMMIT_TOKEN: &str = "title_commit_token";
const COMMIT_MODE: &str = "title_commit_mode";
const COMMIT_ACQUIRED_AT: &str = "title_commit_acquired_at";
const COMMIT_EXPIRES_AT: &str = "title_commit_expires_at";

const TITLE_FIELDS: &[&str] = &[
    TITLE,
    "title_attempt_count",
    "title_next_attempt_at",
    "title_settled",
    claim_data::REGENERATION_LEASE,
    SOURCE,
    SET_AT,
    ATTEMPTED,
    LEASE_HOLDER,
    LEASE_TOKEN,
    LEASE_MODE,
    LEASE_ACQUIRED_AT,
    LEASE_EXPIRES_AT,
    COMMIT_HOLDER,
    COMMIT_TOKEN,
    COMMIT_MODE,
    COMMIT_ACQUIRED_AT,
    COMMIT_EXPIRES_AT,
];

#[derive(Clone)]
pub(super) struct StoredClaim {
    pub(super) holder: String,
    pub(super) token: String,
    pub(super) mode: AgentTitleClaimMode,
    pub(super) acquired_at: DateTime,
    pub(super) expires_at: DateTime,
}

impl StoredClaim {
    pub(super) fn matches_request(&self, request: &AgentTitleClaimRequest) -> bool {
        self.holder == request.holder() && self.token == request.token()
    }

    pub(super) fn matches_claim(&self, claim: &AgentTitleClaim) -> bool {
        self.holder == claim.holder()
            && self.token == claim.token()
            && self.mode == claim.mode()
            && self.acquired_at == DateTime::from_system_time(claim.acquired_at())
            && self.expires_at == DateTime::from_system_time(claim.expires_at())
    }

    pub(super) fn to_claim(&self, request: &AgentTitleClaimRequest) -> AgentTitleClaim {
        AgentTitleClaim::acquired(
            request.clone(),
            self.mode,
            self.acquired_at.to_system_time(),
            self.expires_at.to_system_time(),
        )
    }
}

pub(super) struct TitleState {
    pub(super) retry: super::agent_title_retry::TitleRetry,
    pub(super) lease: Option<StoredClaim>,
    pub(super) stored: Option<StoredAgentTitle>,
    pub(super) completed_claim: Option<StoredClaim>,
    pub(super) user_commit_token: Option<String>,
}

impl TitleState {
    pub(super) fn completed_by(&self, claim: &AgentTitleClaim) -> bool {
        self.completed_claim
            .as_ref()
            .is_some_and(|completed| completed.matches_claim(claim))
    }
}

pub(super) fn initial_title_fields(agent: &Agent) -> Result<Document, SandboxStoreError> {
    let title = agent
        .title
        .as_ref()
        .map(|value| AgentTitle::new(value.clone()))
        .transpose()
        .map_err(|_| SandboxStoreError::Corrupt("agent title is not canonical".into()))?;
    if title.is_some() && seconds_from_rfc3339(&agent.created_at).is_none() {
        return Err(SandboxStoreError::Corrupt(
            "agent title timestamp is invalid".into(),
        ));
    }
    Ok(doc! {
        TITLE: optional_string(title.as_ref().map(AgentTitle::as_str)),
        SOURCE: optional_string(title.as_ref().map(|_| AgentTitleSource::User.as_str())),
        SET_AT: optional_string(title.as_ref().map(|_| agent.created_at.as_str())),
        ATTEMPTED: false,
        claim_data::REGENERATION_LEASE: Bson::Null,
        LEASE_HOLDER: Bson::Null,
        LEASE_TOKEN: Bson::Null,
        LEASE_MODE: Bson::Null,
        LEASE_ACQUIRED_AT: Bson::Null,
        LEASE_EXPIRES_AT: Bson::Null,
        COMMIT_HOLDER: Bson::Null,
        COMMIT_TOKEN: Bson::Null,
        COMMIT_MODE: Bson::Null,
        COMMIT_ACQUIRED_AT: Bson::Null,
        COMMIT_EXPIRES_AT: Bson::Null,
    })
}

pub(super) fn preserve_title_fields(replacement: &mut Document) {
    for field in TITLE_FIELDS {
        replacement.remove(*field);
    }
}

pub(super) fn record_title_value(agent: &Agent) -> Bson {
    optional_string(agent.title.as_deref())
}

pub(super) fn read_title_state(
    document: &Document,
    agent: &Agent,
) -> Result<TitleState, AgentTitleStoreError> {
    let projected_title = optional_stored_string(document, TITLE)?;
    if projected_title
        .as_deref()
        .is_some_and(|title| agent.title.as_deref() != Some(title))
        || projected_title.is_none() && agent.title.is_some() && document.contains_key(TITLE)
    {
        return corrupt();
    }

    let attempted = match document.get(ATTEMPTED) {
        None => false,
        Some(Bson::Boolean(value)) => *value,
        Some(_) => return corrupt(),
    };
    let lease = read_claim(document, ClaimFields::lease())?;
    let (completed_claim, user_commit_token) = read_commit(document)?;
    let source = optional_stored_string(document, SOURCE)?;
    let set_at = optional_stored_string(document, SET_AT)?;
    let stored = match agent.title.as_ref() {
        None => {
            if source.is_some()
                || set_at.is_some()
                || completed_claim.is_some()
                || user_commit_token.is_some()
            {
                return corrupt();
            }
            None
        }
        Some(value) => {
            let title =
                AgentTitle::new(value.clone()).map_err(|_| AgentTitleStoreError::Corrupt)?;
            let source = source
                .as_deref()
                .map(parse_source)
                .transpose()?
                .unwrap_or(AgentTitleSource::User);
            let set_at = set_at.unwrap_or_else(|| agent.created_at.clone());
            if seconds_from_rfc3339(&set_at).is_none() || lease.is_some() {
                return corrupt();
            }
            match source {
                AgentTitleSource::User if completed_claim.is_some() => return corrupt(),
                AgentTitleSource::Generated | AgentTitleSource::Fallback
                    if completed_claim.is_none() || !attempted =>
                {
                    return corrupt();
                }
                AgentTitleSource::Generated
                    if completed_claim
                        .as_ref()
                        .is_some_and(|claim| claim.mode == AgentTitleClaimMode::FallbackOnly) =>
                {
                    return corrupt();
                }
                _ => {}
            }
            Some(StoredAgentTitle {
                agent_id: agent.id.clone(),
                title,
                source,
                set_at,
            })
        }
    };
    let user_commit_token = match stored.as_ref().map(|title| title.source) {
        Some(AgentTitleSource::User) => user_commit_token,
        Some(_) if user_commit_token.is_some() => return corrupt(),
        _ => None,
    };
    let lease = lease.or(claim_data::regeneration_lease(document, stored.as_ref())?);
    if lease.is_some() && !attempted {
        return corrupt();
    }
    Ok(TitleState {
        retry: super::agent_title_retry::read(document)?,
        lease,
        stored,
        completed_claim,
        user_commit_token,
    })
}

pub(super) fn claim_fields(claim: &AgentTitleClaim, regenerating: bool) -> Document {
    let mut fields = super::agent_title_retry::claim_fields(claim);
    fields.insert(ATTEMPTED, true);
    claim_data::write_lease(&mut fields, claim, regenerating);
    fields
}

pub(super) fn lease_filter(claim: &AgentTitleClaim, regenerating: bool) -> Document {
    let mut filter = Document::new();
    insert_claim(&mut filter, ClaimFields::lease(), claim);
    claim_data::lease_filter(filter, regenerating)
}

pub(super) fn automatic_completion_fields(
    claim: &AgentTitleClaim,
    title: &AgentTitle,
    source: AgentTitleSource,
    set_at: &str,
) -> Document {
    let mut fields = completed_fields(title, source, set_at, claim.token());
    fields.insert(
        "title_settled",
        claim.terminal_fallback() || source != AgentTitleSource::Fallback,
    );
    clear_claim(&mut fields, ClaimFields::lease());
    fields.insert(claim_data::REGENERATION_LEASE, Bson::Null);
    insert_claim(&mut fields, ClaimFields::commit(), claim);
    fields
}

pub(super) fn user_completion_fields(
    title: &AgentTitle,
    set_at: &str,
    operation_token: &str,
) -> Document {
    let mut fields = completed_fields(title, AgentTitleSource::User, set_at, operation_token);
    clear_claim(&mut fields, ClaimFields::lease());
    fields.insert(claim_data::REGENERATION_LEASE, Bson::Null);
    clear_claim(&mut fields, ClaimFields::commit());
    fields.insert(COMMIT_TOKEN, operation_token);
    fields
}

fn completed_fields(
    title: &AgentTitle,
    source: AgentTitleSource,
    set_at: &str,
    token: &str,
) -> Document {
    doc! {
        TITLE: title.as_str(),
        SOURCE: source.as_str(),
        SET_AT: set_at,
        COMMIT_TOKEN: token,
    }
}

#[derive(Clone, Copy)]
struct ClaimFields {
    holder: &'static str,
    token: &'static str,
    mode: &'static str,
    acquired_at: &'static str,
    expires_at: &'static str,
}

impl ClaimFields {
    const fn lease() -> Self {
        Self {
            holder: LEASE_HOLDER,
            token: LEASE_TOKEN,
            mode: LEASE_MODE,
            acquired_at: LEASE_ACQUIRED_AT,
            expires_at: LEASE_EXPIRES_AT,
        }
    }

    const fn commit() -> Self {
        Self {
            holder: COMMIT_HOLDER,
            token: COMMIT_TOKEN,
            mode: COMMIT_MODE,
            acquired_at: COMMIT_ACQUIRED_AT,
            expires_at: COMMIT_EXPIRES_AT,
        }
    }
}

fn read_claim(
    document: &Document,
    fields: ClaimFields,
) -> Result<Option<StoredClaim>, AgentTitleStoreError> {
    let holder = optional_stored_string(document, fields.holder)?;
    let token = optional_stored_string(document, fields.token)?;
    let mode = optional_stored_string(document, fields.mode)?;
    let acquired_at = optional_datetime(document, fields.acquired_at)?;
    let expires_at = optional_datetime(document, fields.expires_at)?;
    if [&holder, &token, &mode].iter().all(|value| value.is_none())
        && acquired_at.is_none()
        && expires_at.is_none()
    {
        return Ok(None);
    }
    let (Some(holder), Some(token), Some(mode), Some(acquired_at), Some(expires_at)) =
        (holder, token, mode, acquired_at, expires_at)
    else {
        return corrupt();
    };
    checked_stored_claim(holder, token, mode, acquired_at, expires_at).map(Some)
}

fn read_commit(
    document: &Document,
) -> Result<(Option<StoredClaim>, Option<String>), AgentTitleStoreError> {
    let fields = ClaimFields::commit();
    let holder = optional_stored_string(document, fields.holder)?;
    let token = optional_stored_string(document, fields.token)?;
    let mode = optional_stored_string(document, fields.mode)?;
    let acquired_at = optional_datetime(document, fields.acquired_at)?;
    let expires_at = optional_datetime(document, fields.expires_at)?;
    if holder.is_none() && mode.is_none() && acquired_at.is_none() && expires_at.is_none() {
        if token
            .as_ref()
            .is_some_and(|token| token.is_empty() || token.len() > 256)
        {
            return corrupt();
        }
        return Ok((None, token));
    }
    let (Some(holder), Some(token), Some(mode), Some(acquired_at), Some(expires_at)) =
        (holder, token, mode, acquired_at, expires_at)
    else {
        return corrupt();
    };
    Ok((
        Some(checked_stored_claim(
            holder,
            token,
            mode,
            acquired_at,
            expires_at,
        )?),
        None,
    ))
}

fn checked_stored_claim(
    holder: String,
    token: String,
    mode: String,
    acquired_at: DateTime,
    expires_at: DateTime,
) -> Result<StoredClaim, AgentTitleStoreError> {
    if holder.trim().is_empty()
        || holder.len() > 256
        || token.is_empty()
        || token.len() > 256
        || expires_at
            .timestamp_millis()
            .checked_sub(acquired_at.timestamp_millis())
            != i64::try_from(AGENT_TITLE_CLAIM_SECONDS * 1_000).ok()
    {
        return corrupt();
    }
    Ok(StoredClaim {
        holder,
        token,
        mode: parse_mode(&mode)?,
        acquired_at,
        expires_at,
    })
}

fn insert_claim(document: &mut Document, fields: ClaimFields, claim: &AgentTitleClaim) {
    document.insert(fields.holder, claim.holder());
    document.insert(fields.token, claim.token());
    document.insert(fields.mode, mode_name(claim.mode()));
    document.insert(
        fields.acquired_at,
        DateTime::from_system_time(claim.acquired_at()),
    );
    document.insert(
        fields.expires_at,
        DateTime::from_system_time(claim.expires_at()),
    );
}

fn clear_claim(document: &mut Document, fields: ClaimFields) {
    for field in [
        fields.holder,
        fields.token,
        fields.mode,
        fields.acquired_at,
        fields.expires_at,
    ] {
        document.insert(field, Bson::Null);
    }
}

fn optional_stored_string(
    document: &Document,
    field: &str,
) -> Result<Option<String>, AgentTitleStoreError> {
    match document.get(field) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::String(value)) => Ok(Some(value.clone())),
        Some(_) => corrupt(),
    }
}

fn optional_datetime(
    document: &Document,
    field: &str,
) -> Result<Option<DateTime>, AgentTitleStoreError> {
    match document.get(field) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::DateTime(value)) => Ok(Some(*value)),
        Some(_) => corrupt(),
    }
}

fn optional_string(value: Option<&str>) -> Bson {
    value.map(Bson::from).unwrap_or(Bson::Null)
}

fn parse_source(value: &str) -> Result<AgentTitleSource, AgentTitleStoreError> {
    match value {
        "user" => Ok(AgentTitleSource::User),
        "generated" => Ok(AgentTitleSource::Generated),
        "fallback" => Ok(AgentTitleSource::Fallback),
        _ => corrupt(),
    }
}

fn parse_mode(value: &str) -> Result<AgentTitleClaimMode, AgentTitleStoreError> {
    match value {
        "generate" => Ok(AgentTitleClaimMode::Generate),
        "fallback_only" => Ok(AgentTitleClaimMode::FallbackOnly),
        _ => corrupt(),
    }
}

fn mode_name(mode: AgentTitleClaimMode) -> &'static str {
    match mode {
        AgentTitleClaimMode::Generate => "generate",
        AgentTitleClaimMode::FallbackOnly => "fallback_only",
    }
}

fn corrupt<T>() -> Result<T, AgentTitleStoreError> {
    Err(AgentTitleStoreError::Corrupt)
}

#[cfg(test)]
#[path = "agent_title_rollback_tests.rs"]
mod rollback_tests;

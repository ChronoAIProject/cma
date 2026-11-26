//! Exercise persisted producer output against develop's pre-regeneration reader.
use super::super::agents::{agent_document, agent_replacement_fields, read_agent};
use super::*;
use layer2_core::ids::{AgentId, AgentProfileId, SandboxId, SubjectId, WorkspaceId, WorktreeId};
use layer2_core::profiles::{
    AgentProfileContent, AgentProfileSnapshot, PermissionLevel, ThinkingEffort,
};
use layer2_core::sandbox::{
    AgentOrigin, AgentStatus, AgentWorkState, AgentWorkStep, ConversationState,
};
use layer2_core::{RuntimeCapabilities, RuntimeCapability};
use std::time::{Duration, SystemTime};

const NOW: &str = "2026-09-05T00:00:00Z";
const REGEN: &str = "title_regeneration_lease";

struct LegacyTitleState {
    attempted: bool,
    lease: Option<StoredClaim>,
    stored: Option<StoredAgentTitle>,
    completed_claim: Option<StoredClaim>,
    user_commit_token: Option<String>,
}

// Copied from origin/develop at 6750dac2a, agent_title_state::read_title_state.
// Only function/return names and the explicit old lease-key selector differ.
// The BSON decoders, field names and 45-second duration below are frozen from that reader.
// No current retry/regeneration reader or claim decoder participates in this compatibility check.
fn read_legacy_title_state(
    document: &Document,
    agent: &Agent,
) -> Result<LegacyTitleState, AgentTitleStoreError> {
    let projected_title = legacy_optional_stored_string(document, TITLE)?;
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
    let lease = legacy_read_claim(
        document,
        ClaimFields {
            holder: "title_lease_holder",
            token: "title_lease_token",
            mode: "title_lease_mode",
            acquired_at: "title_lease_acquired_at",
            expires_at: "title_lease_expires_at",
        },
    )?;
    if lease.is_some() && !attempted {
        return corrupt();
    }

    let (completed_claim, user_commit_token) = legacy_read_commit(document)?;
    let source = legacy_optional_stored_string(document, SOURCE)?;
    let set_at = legacy_optional_stored_string(document, SET_AT)?;
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
                .map(legacy_parse_source)
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
    Ok(LegacyTitleState {
        attempted,
        lease,
        stored,
        completed_claim,
        user_commit_token,
    })
}

fn legacy_read_claim(
    document: &Document,
    fields: ClaimFields,
) -> Result<Option<StoredClaim>, AgentTitleStoreError> {
    let holder = legacy_optional_stored_string(document, fields.holder)?;
    let token = legacy_optional_stored_string(document, fields.token)?;
    let mode = legacy_optional_stored_string(document, fields.mode)?;
    let acquired_at = legacy_optional_datetime(document, fields.acquired_at)?;
    let expires_at = legacy_optional_datetime(document, fields.expires_at)?;
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
    legacy_checked_stored_claim(holder, token, mode, acquired_at, expires_at).map(Some)
}

fn legacy_read_commit(
    document: &Document,
) -> Result<(Option<StoredClaim>, Option<String>), AgentTitleStoreError> {
    let fields = ClaimFields {
        holder: "title_commit_holder",
        token: "title_commit_token",
        mode: "title_commit_mode",
        acquired_at: "title_commit_acquired_at",
        expires_at: "title_commit_expires_at",
    };
    let holder = legacy_optional_stored_string(document, fields.holder)?;
    let token = legacy_optional_stored_string(document, fields.token)?;
    let mode = legacy_optional_stored_string(document, fields.mode)?;
    let acquired_at = legacy_optional_datetime(document, fields.acquired_at)?;
    let expires_at = legacy_optional_datetime(document, fields.expires_at)?;
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
        Some(legacy_checked_stored_claim(
            holder,
            token,
            mode,
            acquired_at,
            expires_at,
        )?),
        None,
    ))
}

fn legacy_checked_stored_claim(
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
            != i64::try_from(45_u64 * 1_000).ok()
    {
        return corrupt();
    }
    Ok(StoredClaim {
        holder,
        token,
        mode: legacy_parse_mode(&mode)?,
        acquired_at,
        expires_at,
    })
}

fn legacy_optional_stored_string(
    document: &Document,
    field: &str,
) -> Result<Option<String>, AgentTitleStoreError> {
    match document.get(field) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::String(value)) => Ok(Some(value.clone())),
        Some(_) => corrupt(),
    }
}

fn legacy_optional_datetime(
    document: &Document,
    field: &str,
) -> Result<Option<DateTime>, AgentTitleStoreError> {
    match document.get(field) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::DateTime(value)) => Ok(Some(*value)),
        Some(_) => corrupt(),
    }
}

fn legacy_parse_source(value: &str) -> Result<AgentTitleSource, AgentTitleStoreError> {
    match value {
        "user" => Ok(AgentTitleSource::User),
        "generated" => Ok(AgentTitleSource::Generated),
        "fallback" => Ok(AgentTitleSource::Fallback),
        _ => corrupt(),
    }
}

fn legacy_parse_mode(value: &str) -> Result<AgentTitleClaimMode, AgentTitleStoreError> {
    match value {
        "generate" => Ok(AgentTitleClaimMode::Generate),
        "fallback_only" => Ok(AgentTitleClaimMode::FallbackOnly),
        _ => corrupt(),
    }
}

fn claim(attempts: u32, elapsed: u64) -> AgentTitleClaim {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000_000 + elapsed);
    let request = AgentTitleClaimRequest::new(AgentId("agt_title".into()), "title-worker");
    let expires = now + request.ttl();
    AgentTitleClaim::acquired(request, AgentTitleClaimMode::Generate, now, expires)
        .with_attempts(attempts)
}

fn assert_readable(document: &Document) {
    let agent = read_agent(document).unwrap();
    let old = read_legacy_title_state(document, &agent).unwrap();
    let current = read_title_state(document, &agent).unwrap();
    assert_eq!(
        old.stored
            .as_ref()
            .map(|title| (&title.title, title.source)),
        current
            .stored
            .as_ref()
            .map(|title| (&title.title, title.source))
    );
    assert_eq!(
        old.completed_claim.is_some(),
        current.completed_claim.is_some()
    );
    assert_eq!(old.user_commit_token, current.user_commit_token);
    if current.lease.is_some() {
        assert!(old.attempted);
    }
    if agent.title.is_some() {
        assert!(old.lease.is_none());
    }
}

fn complete(document: &mut Document, claim: &AgentTitleClaim, source: AgentTitleSource) {
    let title = AgentTitle::new(if source == AgentTitleSource::Fallback {
        "Retained fallback"
    } else {
        "Generated title"
    })
    .unwrap();
    let mut agent = read_agent(document).unwrap();
    agent.title = Some(title.as_str().to_owned());
    agent.revision += 1;
    agent.conversation.last_seq += 1;
    document.insert(
        "last_seq",
        i64::try_from(agent.conversation.last_seq).unwrap(),
    );
    document.extend(agent_replacement_fields(&agent).unwrap());
    document.extend(automatic_completion_fields(claim, &title, source, NOW));
}

fn fallback() -> Document {
    let mut document = agent_document(&agent()).unwrap();
    let initial = claim(0, 0);
    document.extend(claim_fields(&initial, false));
    complete(&mut document, &initial, AgentTitleSource::Fallback);
    // Historical documents have none of the new retry/regeneration metadata.
    for field in [
        REGEN,
        "title_attempt_count",
        "title_next_attempt_at",
        "title_settled",
    ] {
        document.remove(field);
    }
    assert_readable(&document);
    document
}

fn matches(document: &Document, filter: &Document) -> bool {
    filter.iter().all(|(key, expected)| {
        let mut parts = key.split('.');
        let first = parts.next().unwrap();
        let value = parts.try_fold(document.get(first), |value, part| {
            value?.as_document()?.get(part).map(Some)
        });
        value.flatten() == Some(expected)
    })
}

#[test]
fn legacy_reader_rejects_the_previous_title_and_legacy_lease_combination() {
    let mut document = fallback();
    let request = claim(1, 60);
    let mut fields = Document::new();
    insert_claim(&mut fields, ClaimFields::lease(), &request);
    document.extend(fields);
    let agent = read_agent(&document).unwrap();
    assert!(matches!(
        read_legacy_title_state(&document, &agent),
        Err(AgentTitleStoreError::Corrupt)
    ));
    assert!(matches!(
        read_title_state(&document, &agent),
        Err(AgentTitleStoreError::Corrupt)
    ));
}

#[test]
fn fresh_claim_failure_and_crash_states_remain_readable_without_a_title() {
    let mut document = agent_document(&agent()).unwrap();
    for attempts in 1..=3 {
        let request = claim(attempts, 240 * u64::from(attempts));
        document.extend(claim_fields(&request, false));
        assert_readable(&document);
        assert!(matches(&document, &lease_filter(&request, false)));
        let state = read_title_state(&document, &read_agent(&document).unwrap()).unwrap();
        assert!(!state
            .retry
            .eligible(DateTime::from_system_time(request.acquired_at())));
        assert!(state
            .retry
            .eligible(DateTime::from_system_time(request.next_attempt_at())));
        assert_eq!(document.get(REGEN), Some(&Bson::Null));
        // Failure, skip and process death persist this same claimed state; no cleanup is required.
        assert_readable(&document.clone());
    }
}

#[test]
fn fallback_regeneration_survives_backoff_crashes_and_an_indefinitely_paused_parent() {
    let mut document = fallback();
    let mut previous = None;
    for attempts in 1..=3 {
        let request = claim(attempts, 240 * u64::from(attempts));
        document.extend(claim_fields(&request, true));
        assert_readable(&document);
        assert!(matches(&document, &lease_filter(&request, true)));
        assert!(!matches(&document, &lease_filter(&request, false)));
        if let Some(old) = previous.as_ref() {
            assert!(!matches(&document, &lease_filter(old, true)));
        }
        let state = read_title_state(&document, &read_agent(&document).unwrap()).unwrap();
        assert!(state.lease.as_ref().unwrap().matches_claim(&request));
        assert_eq!(state.retry.attempts, attempts);
        assert!(!state
            .retry
            .eligible(DateTime::from_system_time(request.acquired_at())));
        assert!(state
            .retry
            .eligible(DateTime::from_system_time(request.next_attempt_at())));
        // Ordinary Agent writes, including pause/stop, preserve the separate lease and budget.
        let mut stopped = read_agent(&document).unwrap();
        stopped.status = AgentStatus::Stopped;
        stopped.revision += 1;
        let envelope = document.get(REGEN).cloned();
        document.extend(agent_replacement_fields(&stopped).unwrap());
        assert_eq!(document.get(REGEN), envelope.as_ref());
        assert_readable(&document);
        let resumed = read_title_state(&document, &stopped).unwrap();
        assert!(resumed.retry.eligible(DateTime::from_system_time(
            request.next_attempt_at() + Duration::from_secs(86_400)
        )));
        previous = Some(request);
    }
    let last = previous.unwrap();
    complete(
        &mut document,
        &last.settle_fallback(),
        AgentTitleSource::Fallback,
    );
    assert_readable(&document);
    assert_eq!(document.get(REGEN), Some(&Bson::Null));
    assert!(
        read_title_state(&document, &read_agent(&document).unwrap())
            .unwrap()
            .retry
            .settled
    );
}

#[test]
fn an_expired_final_attempt_recovers_with_a_fallback_only_claim() {
    let mut document = fallback();
    let failed = claim(3, 240);
    document.extend(claim_fields(&failed, true));
    assert_readable(&document);
    let request = AgentTitleClaimRequest::new(failed.agent_id().clone(), "restarted-worker");
    let now = failed.next_attempt_at();
    let expires = now + request.ttl();
    let recovery =
        AgentTitleClaim::acquired(request, AgentTitleClaimMode::FallbackOnly, now, expires)
            .with_attempts(3)
            .settle_fallback();
    document.extend(claim_fields(&recovery, true));
    assert_readable(&document);
    assert!(!matches(&document, &lease_filter(&failed, true)));
    complete(&mut document, &recovery, AgentTitleSource::Fallback);
    assert_readable(&document);
    assert!(
        read_title_state(&document, &read_agent(&document).unwrap())
            .unwrap()
            .retry
            .settled
    );
}

#[test]
fn generated_and_permanent_failure_settlement_clear_both_lease_namespaces() {
    for source in [AgentTitleSource::Generated, AgentTitleSource::Fallback] {
        let mut document = fallback();
        let request = claim(1, 60).settle_fallback();
        document.extend(claim_fields(&request, true));
        complete(&mut document, &request, source);
        assert_readable(&document);
        let state = read_title_state(&document, &read_agent(&document).unwrap()).unwrap();
        assert!(state.retry.settled);
        assert!(state.lease.is_none());
        assert!(state.completed_by(&request));
        assert_eq!(document.get(REGEN), Some(&Bson::Null));
    }
}

#[test]
fn owner_rename_during_regeneration_stays_readable_and_wins_over_later_completion() {
    let mut document = fallback();
    let request = claim(1, 60);
    document.extend(claim_fields(&request, true));
    let mut renamed = read_agent(&document).unwrap();
    renamed.conversation.preferences.title = Some("Owner's chosen name".into());
    renamed.conversation.preferences.revision += 1;
    renamed.revision += 1;
    document.extend(agent_replacement_fields(&renamed).unwrap());
    assert_readable(&document);
    complete(&mut document, &request, AgentTitleSource::Generated);
    assert_readable(&document);
    assert_eq!(
        read_agent(&document)
            .unwrap()
            .conversation
            .preferences
            .title
            .as_deref(),
        Some("Owner's chosen name")
    );
}

#[test]
fn a_user_title_fences_an_envelope_left_by_an_older_writer() {
    let mut document = fallback();
    let request = claim(1, 60);
    document.extend(claim_fields(&request, true));
    let mut renamed = read_agent(&document).unwrap();
    let title = AgentTitle::new("User title").unwrap();
    renamed.title = Some(title.as_str().into());
    renamed.revision += 1;
    document.extend(agent_replacement_fields(&renamed).unwrap());
    let mut old_write = user_completion_fields(&title, NOW, "owner-operation");
    old_write.remove(REGEN); // The rollback writer does not know this field.
    document.extend(old_write);
    assert_readable(&document);
    assert!(read_title_state(&document, &renamed)
        .unwrap()
        .lease
        .is_none());
    assert!(document.get_document(REGEN).is_ok());
    document.extend(user_completion_fields(&title, NOW, "owner-operation"));
    assert_eq!(document.get(REGEN), Some(&Bson::Null));
    assert_readable(&document);
}

#[test]
fn malformed_regeneration_authority_is_refused_without_corrupting_the_old_reader() {
    let mut document = fallback();
    document.insert(REGEN, doc! { "title_lease_token": "incomplete" });
    let agent = read_agent(&document).unwrap();
    assert!(read_legacy_title_state(&document, &agent).is_ok());
    assert!(matches!(
        read_title_state(&document, &agent),
        Err(AgentTitleStoreError::Corrupt)
    ));
}

fn agent() -> Agent {
    Agent {
        id: AgentId("agt_title".into()),
        worktree_id: WorktreeId("wkt_title".into()),
        workspace_id: WorkspaceId("wks_title".into()),
        sandbox_id: SandboxId("sbx_title".into()),
        owner: SubjectId::new("owner"),
        creation_key: "title-agent".into(),
        creation_fingerprint: "a".repeat(64),
        profile: AgentProfileSnapshot {
            id: AgentProfileId("agp_title".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: AgentProfileContent {
                isolated_agent: false,
                model: Some("agent-selected-model".into()),
                thinking_effort: ThinkingEffort::Medium,
                permission_level: PermissionLevel::Guarded,
                skills: vec![],
                system_prompt: String::new(),
            },
        },
        capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        status: AgentStatus::Ready,
        conversation: ConversationState {
            turn_timeout: None,
            preferences: Default::default(),
            managed_guidance_revision: 0,
            observations: Default::default(),
            settings: None,
            conversation_id: Some("conversation-title".into()),
            runtime_generation: Some(1),
            last_seq: 0,
            first_message: None,
            active_turn: None,
            operation: None,
            queued_messages: vec![],
        },
        work: AgentWorkState::new(AgentWorkStep::None, NOW.into()),
        failure: None,
        title: None,
        created_at: NOW.into(),
        updated_at: NOW.into(),
        ready_at: Some(NOW.into()),
        stopped_at: None,
        requested_stop_reason: None,
        stop_reason: None,
        deleted_at: None,
        origin: AgentOrigin::Manual,
        revision: 1,
    }
}

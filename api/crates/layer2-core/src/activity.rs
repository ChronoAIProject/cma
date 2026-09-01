//! What the service was asked to do, by whom, and how it came out.
//!
//! The vocabulary is deliberately small: an actor, an action, a target, an
//! outcome, and the parameters worth keeping. Nothing here knows how a request
//! reached the service — the surface that received it translates into these
//! terms, so the record reads the same whichever way somebody called.
//!
//! Which parameters may be kept is not decided here. That is an allowlist, and
//! it lives in [`crate::activity_detail`] so the set can be read on its own
//! and reviewed as the security boundary it is.

use serde::{Deserialize, Serialize};

use crate::activity_detail::{ActivityDetail, DetailKey};
use crate::clock::now_rfc3339;
use crate::ids::{EnvironmentProfileId, SessionId, SubjectId};
use crate::trigger::{LaunchId, PublicTriggerId, TriggerId, TriggerRevisionId};

/// Most entries one read may return.
pub const MAX_ACTIVITY_LIMIT: usize = 500;

/// How many entries a read returns when the caller does not say.
pub const DEFAULT_ACTIVITY_LIMIT: usize = 100;

/// Identifier of one recorded action.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActivityId(pub String);

impl ActivityId {
    /// Mint a fresh identifier.
    pub fn generate() -> Self {
        Self(format!("actv_{}", uuid::Uuid::new_v4().simple()))
    }

    /// Borrow the underlying string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ActivityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Something the service can be asked to do.
///
/// A closed set rather than free text. An open one would fill with whatever
/// string the surface that received the request happened to have to hand, and
/// a log nobody can group or filter is a log nobody reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityAction {
    /// A subject established a signed-in identity.
    SignIn,
    /// A subject gave up their signed-in identity.
    SignOut,
    /// A session was provisioned.
    SessionCreate,
    /// One session was read.
    SessionRead,
    /// Sessions were listed.
    SessionList,
    /// A session's runtime was released, keeping the session.
    SessionStop,
    /// A session was ended.
    SessionTerminate,
    /// A session was given a runtime again.
    SessionRevive,
    /// A session and its history were removed.
    SessionDelete,
    /// A session's rendered screen was read.
    SessionScreenRead,
    /// Everything a session has shown was read.
    SessionTranscriptRead,
    /// A session's durable history was read.
    SessionEventsRead,
    /// A client took the exclusive terminal attachment.
    SessionTerminalAttach,
    /// Something was sent to a session — keystrokes, or a prompt turn.
    ///
    /// What was sent is deliberately not part of this record. It is the
    /// owner's own words to their own agent, and the durable event log is
    /// where a session's content belongs; this log is about who did what.
    SessionInput,
    /// A turn or a running command was stopped without ending the session.
    SessionInterrupt,
    /// A turn already in progress was redirected.
    SessionSteer,
    /// A session's terminal was told its geometry changed.
    SessionResize,
    /// A session was moved onto the other plane.
    ///
    /// Worth its own action rather than folding into a rotation: this is the
    /// one handover somebody asked for, and what they asked for is the
    /// question a record of who did what exists to answer.
    SessionPlaneChange,
    /// Something the runtime was blocked on was answered.
    ///
    /// That it was answered is recorded; which way is not. The decision is in
    /// the session's own event log, where it belongs to the conversation
    /// rather than to the audit of who touched what.
    SessionApprovalResolve,
    /// A session's accumulated usage was read.
    SessionUsageRead,
    /// Deployment-managed chat hosts were listed.
    ChatHostList,
    /// One deployment-managed chat host was retired.
    ChatHostRetire,
    /// A reusable environment bundle was created.
    EnvironmentProfileCreate,
    /// One bundle was read.
    EnvironmentProfileRead,
    /// Bundles were listed.
    EnvironmentProfileList,
    /// A bundle's contents were replaced.
    EnvironmentProfileReplace,
    /// A bundle was removed.
    EnvironmentProfileDelete,
    /// The selectable skills were listed.
    SkillCatalogRead,
    /// The runtimes in service were listed.
    RuntimeInventoryRead,
    /// The directory of subjects was read.
    UserDirectoryRead,
    /// This record itself was read.
    ActivityLogRead,
    /// The deployment's settings were read.
    PlatformSettingsRead,
    /// One or more deployment settings were written.
    ///
    /// Which settings changed is recorded separately; their values are not.
    PlatformSettingsWrite,
    /// The session briefing was restored to its shipped text.
    PlatformSettingsReset,
    TriggerPreviewRead,
    TriggerDraftCreate,
    TriggerDraftRead,
    TriggerDraftList,
    TriggerDraftUpdate,
    TriggerRevisionPublish,
    TriggerRevisionRead,
    TriggerRevisionList,
    TriggerRevisionRetire,
    TriggerConnectionList,
    TriggerConnectionBegin,
    TriggerConnectionRevoke,
    TriggerConnectionReconnect,
    TriggerConnectionPreflight,
    TriggerRepositoryResolve,
    TriggerLaunchCreate,
    TriggerLaunchRead,
    TriggerLaunchList,
    TriggerLaunchDraftUpdate,
    TriggerLaunchConnectionSelect,
    TriggerLaunchStart,
    TriggerLaunchDiffReview,
    TriggerLaunchDiffRead,
    TriggerLaunchContributionPublish,
    TriggerLaunchPullRequestReview,
    TriggerLaunchPullRequestOpen,
    TriggerLaunchPullRequestUpdate,
    TriggerLaunchDiffDownload,
    TriggerLaunchCancel,
    TriggerLaunchPromptRecover,
    TriggerLaunchEventsRead,
    TriggerLaunchSupportRead,
    TriggerOrchestrationPause,
    TriggerOrchestrationResume,
    TriggerEffectExecute,
    TriggerEffectReconcile,
    TriggerEffectRecoveryAuthorize,
    TriggerCapabilityRenew,
    TriggerCapabilityRevoke,
    TriggerCleanupPlan,
    TriggerCleanupExecute,
}

impl ActivityAction {
    /// Whether this action belongs to the CMA Trigger workflow.
    ///
    /// Kept as an exhaustive match so adding a Trigger action forces its audit
    /// and alert behavior to be considered rather than relying on a string
    /// prefix after serialization.
    pub fn is_trigger(self) -> bool {
        matches!(
            self,
            Self::TriggerPreviewRead
                | Self::TriggerDraftCreate
                | Self::TriggerDraftRead
                | Self::TriggerDraftList
                | Self::TriggerDraftUpdate
                | Self::TriggerRevisionPublish
                | Self::TriggerRevisionRead
                | Self::TriggerRevisionList
                | Self::TriggerRevisionRetire
                | Self::TriggerConnectionList
                | Self::TriggerConnectionBegin
                | Self::TriggerConnectionRevoke
                | Self::TriggerConnectionReconnect
                | Self::TriggerConnectionPreflight
                | Self::TriggerRepositoryResolve
                | Self::TriggerLaunchCreate
                | Self::TriggerLaunchRead
                | Self::TriggerLaunchList
                | Self::TriggerLaunchDraftUpdate
                | Self::TriggerLaunchConnectionSelect
                | Self::TriggerLaunchStart
                | Self::TriggerLaunchDiffReview
                | Self::TriggerLaunchDiffRead
                | Self::TriggerLaunchContributionPublish
                | Self::TriggerLaunchPullRequestReview
                | Self::TriggerLaunchPullRequestOpen
                | Self::TriggerLaunchPullRequestUpdate
                | Self::TriggerLaunchDiffDownload
                | Self::TriggerLaunchCancel
                | Self::TriggerLaunchPromptRecover
                | Self::TriggerLaunchEventsRead
                | Self::TriggerLaunchSupportRead
                | Self::TriggerOrchestrationPause
                | Self::TriggerOrchestrationResume
                | Self::TriggerEffectExecute
                | Self::TriggerEffectReconcile
                | Self::TriggerEffectRecoveryAuthorize
                | Self::TriggerCapabilityRenew
                | Self::TriggerCapabilityRevoke
                | Self::TriggerCleanupPlan
                | Self::TriggerCleanupExecute
        )
    }
}

/// What an action was taken on.
///
/// Absent for actions that address a collection rather than one thing: listing
/// sessions has no target, and inventing one would be a lie about what was
/// asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ActivityTarget {
    /// One session, by identifier.
    Session(SessionId),
    /// One deployment-managed chat host, by identifier.
    ChatHost(crate::ChatHostId),
    /// One reusable environment bundle, by identifier.
    EnvironmentProfile(EnvironmentProfileId),
    /// One subject, when the action concerns a person rather than a resource.
    Subject(SubjectId),
    /// One mutable publisher Trigger draft.
    Trigger(TriggerId),
    /// One immutable Trigger revision.
    TriggerRevision(TriggerRevisionId),
    /// One public README-facing Trigger identifier.
    PublicTrigger(PublicTriggerId),
    /// One contributor Launch.
    Launch(LaunchId),
    /// One exact NyxID GitHub UserService identifier.
    TriggerConnection(String),
}

impl ActivityTarget {
    /// The identifier of whatever was addressed.
    pub fn id(&self) -> &str {
        match self {
            Self::Session(id) => id.as_str(),
            Self::ChatHost(id) => id.as_str(),
            Self::EnvironmentProfile(id) => id.as_str(),
            Self::Subject(id) => id.as_str(),
            Self::Trigger(id) => id.0.as_str(),
            Self::TriggerRevision(id) => id.0.as_str(),
            Self::PublicTrigger(id) => id.0.as_str(),
            Self::Launch(id) => id.0.as_str(),
            Self::TriggerConnection(id) => id.as_str(),
        }
    }
}

/// How an action came out.
///
/// Four answers rather than a status code, because the distinction that
/// matters to whoever reads this is whether the caller was turned away, sent
/// something unusable, or hit a failure that was not their doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityOutcome {
    /// It was carried out.
    Succeeded,
    /// Refused: the caller was not entitled to it.
    Denied,
    /// Refused: the request described something unusable.
    Rejected,
    /// Attempted, and failed for a reason that was not the caller's doing.
    Failed,
}

impl ActivityOutcome {
    /// Whether this outcome is worth an operator's attention on its own.
    pub fn is_notable(self) -> bool {
        matches!(self, Self::Denied | Self::Failed)
    }
}

/// One thing the service was asked to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityRecord {
    /// Identifier of this entry.
    pub id: ActivityId,
    /// When it happened, RFC 3339 in UTC.
    pub at: String,
    /// Who asked.
    ///
    /// Absent when the request carried no established identity. A refused
    /// caller is worth recording precisely because they were refused, and
    /// attributing them to somebody would be worse than recording an absence.
    pub actor: Option<SubjectId>,
    /// What they asked for.
    pub action: ActivityAction,
    /// What they asked for it on.
    pub target: Option<ActivityTarget>,
    /// How it came out.
    pub outcome: ActivityOutcome,
    /// The parameters worth keeping.
    pub detail: ActivityDetail,
}

impl ActivityRecord {
    /// Begin a record for an action and its outcome.
    ///
    /// The identifier and the timestamp are minted here rather than by the
    /// store, so an entry is complete before anything durable is involved and
    /// two stores cannot disagree about what an identifier looks like.
    pub fn new(action: ActivityAction, outcome: ActivityOutcome) -> Self {
        Self {
            id: ActivityId::generate(),
            at: now_rfc3339(),
            actor: None,
            action,
            target: None,
            outcome,
            detail: ActivityDetail::new(),
        }
    }

    /// Attribute the action to a subject.
    pub fn by(mut self, actor: SubjectId) -> Self {
        self.actor = Some(actor);
        self
    }

    /// Say what the action was taken on.
    pub fn on(mut self, target: ActivityTarget) -> Self {
        self.target = Some(target);
        self
    }

    /// Record one allowlisted parameter.
    pub fn with(mut self, key: DetailKey, value: impl Into<String>) -> Self {
        self.detail.record(key, value);
        self
    }
}

/// Which entries a read wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityQuery {
    /// Only entries attributed to this subject.
    pub actor: Option<SubjectId>,
    /// Most entries to return, newest first.
    pub limit: usize,
}

impl Default for ActivityQuery {
    fn default() -> Self {
        Self {
            actor: None,
            limit: DEFAULT_ACTIVITY_LIMIT,
        }
    }
}

impl ActivityQuery {
    /// Bring the limit inside what a read may ask for.
    ///
    /// Clamped rather than refused: a caller asking for more than the log will
    /// hand over wants as much as they can get, and failing the request would
    /// tell them nothing they can act on.
    pub fn clamped(mut self) -> Self {
        self.limit = self.limit.clamp(1, MAX_ACTIVITY_LIMIT);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_identifiers_are_unique_and_prefixed() {
        let a = ActivityId::generate();
        let b = ActivityId::generate();
        assert_ne!(a, b);
        assert!(a.as_str().starts_with("actv_"), "got {a}");
    }

    #[test]
    fn a_record_serialises_with_everything_a_reader_needs() {
        let record =
            ActivityRecord::new(ActivityAction::SessionTerminate, ActivityOutcome::Succeeded)
                .by(SubjectId("operator".into()))
                .on(ActivityTarget::Session(SessionId("sesn_1".into())))
                .with(DetailKey::Runtime, "example-runtime");

        let json = serde_json::to_value(&record).expect("serialises");
        assert_eq!(json["action"], "session_terminate");
        assert_eq!(json["outcome"], "succeeded");
        assert_eq!(json["actor"], "operator");
        assert_eq!(json["target"]["kind"], "session");
        assert_eq!(json["target"]["id"], "sesn_1");
        assert_eq!(json["detail"]["runtime"], "example-runtime");
        assert!(json["at"].as_str().expect("a timestamp").ends_with('Z'));
    }

    #[test]
    fn a_record_round_trips_through_its_serialised_form() {
        let record = ActivityRecord::new(ActivityAction::SessionCreate, ActivityOutcome::Denied)
            .with(DetailKey::PermissionMode, "guarded");
        let json = serde_json::to_string(&record).expect("serialises");
        let back: ActivityRecord = serde_json::from_str(&json).expect("parses");
        assert_eq!(back, record);
        assert!(back.actor.is_none(), "an unattributed entry stays so");
        assert!(back.target.is_none());
    }

    #[test]
    fn an_unattributed_record_is_recorded_rather_than_attributed_to_somebody() {
        let record = ActivityRecord::new(ActivityAction::SessionList, ActivityOutcome::Denied);
        assert!(record.actor.is_none());
        assert!(record.detail.is_empty());
    }

    #[test]
    fn a_query_limit_is_brought_inside_what_a_read_will_serve() {
        assert_eq!(
            ActivityQuery {
                actor: None,
                limit: 100_000
            }
            .clamped()
            .limit,
            MAX_ACTIVITY_LIMIT
        );
        assert_eq!(
            ActivityQuery {
                actor: None,
                limit: 0
            }
            .clamped()
            .limit,
            1,
            "asking for nothing must not silently mean asking for everything"
        );
        assert_eq!(ActivityQuery::default().limit, DEFAULT_ACTIVITY_LIMIT);
    }

    #[test]
    fn refusals_and_failures_are_the_outcomes_worth_noticing() {
        assert!(ActivityOutcome::Denied.is_notable());
        assert!(ActivityOutcome::Failed.is_notable());
        assert!(!ActivityOutcome::Succeeded.is_notable());
        assert!(!ActivityOutcome::Rejected.is_notable());
    }

    #[test]
    fn a_target_reports_its_identifier_whatever_kind_it_is() {
        assert_eq!(
            ActivityTarget::Session(SessionId("sesn_1".into())).id(),
            "sesn_1"
        );
        assert_eq!(
            ActivityTarget::EnvironmentProfile(EnvironmentProfileId("envp_1".into())).id(),
            "envp_1"
        );
        assert_eq!(
            ActivityTarget::Subject(SubjectId("subject-a".into())).id(),
            "subject-a"
        );
    }

    #[test]
    fn trigger_actions_are_classified_without_string_inference() {
        assert!(ActivityAction::TriggerPreviewRead.is_trigger());
        assert!(ActivityAction::TriggerCleanupExecute.is_trigger());
        assert!(!ActivityAction::SessionCreate.is_trigger());
        assert!(!ActivityAction::ActivityLogRead.is_trigger());
    }
}

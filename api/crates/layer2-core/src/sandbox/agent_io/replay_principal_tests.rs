use super::*;
use crate::sandbox::control::{SandboxControlEnvelope, SandboxControlError};
use crate::sandbox::AgentOperationKind;

const LEGACY_AUTHORITY: &str =
    r#"{"authority_id":"authority-a","policy_revision":1,"grant_revision":null}"#;
const PRINCIPAL_AUTHORITY: &str = r#"{"authority_id":"authority-a","policy_revision":1,"grant_revision":null,"replay_principal":"principal-a"}"#;
const LEGACY_COMMAND: &str = r#"{"schema_version":1,"command_id":"command","agent_id":"agent","owner":{"kind":"person","subject":"owner"},"actor":{"kind":"human","subject":"owner"},"source":{"kind":"api","reference":"request"},"correlation_id":"correlation","request_fingerprint":"59caf2610b09b536f3056230ea497b355e3c287f421549172bdb34fa6dae6a67","authorization":{"authority_id":"authority-a","policy_revision":1,"grant_revision":null},"command":{"type":"message","input":{"content":[{"type":"text","text":"hello"}]},"order":{"type":"append"},"metadata":{}}}"#;
const LEGACY_CONTROL: &str = r#"{"command_id":"control","sandbox_id":"sandbox","sandbox_created_at":"2026-01-01T00:00:00Z","owner":{"kind":"person","subject":"owner"},"actor":{"kind":"human","subject":"owner"},"authorization":{"authority_id":"authority-a","policy_revision":1,"grant_revision":null},"command":"pause","expected_revision":1,"request_fingerprint":"dba729fabd46f3af457ed41ab20f10cd738dffba4c9335eb0319c99d44d05c4e"}"#;

fn authority() -> AgentAuthorizationRevision {
    serde_json::from_str(LEGACY_AUTHORITY).unwrap()
}
fn command() -> AgentCommandEnvelope {
    serde_json::from_str(LEGACY_COMMAND).unwrap()
}
fn control() -> SandboxControlEnvelope {
    serde_json::from_str(LEGACY_CONTROL).unwrap()
}

#[test]
fn legacy_scope_goldens() {
    // Captured by running these scope functions before the optional field existed.
    assert_eq!(
        authority().idempotency_scope(&AgentId("agent".into()), AgentOperationKind::SendMessage),
        "90484d3f4e258d31ae7716dcdd2400a811ce4450c48ed3a144122fd2e97bb157"
    );
    assert_eq!(
        control().scope(),
        "6458e0d0719a0335fdc747578446d978758ed68714f8e5b9ac3a32142b95fd83"
    );
}

#[test]
fn authority_and_full_envelopes_round_trip_old_and_new_json_bytes() {
    for authorization in [LEGACY_AUTHORITY, PRINCIPAL_AUTHORITY] {
        let revision: AgentAuthorizationRevision = serde_json::from_str(authorization).unwrap();
        assert!(revision.valid_replay_principal());
        assert_eq!(serde_json::to_string(&revision).unwrap(), authorization);
        let encoded = LEGACY_COMMAND.replace(LEGACY_AUTHORITY, authorization);
        let command: AgentCommandEnvelope = serde_json::from_str(&encoded).unwrap();
        command.validate_integrity().unwrap();
        assert_eq!(serde_json::to_string(&command).unwrap(), encoded);
        let encoded = LEGACY_CONTROL.replace(LEGACY_AUTHORITY, authorization);
        let control: SandboxControlEnvelope = serde_json::from_str(&encoded).unwrap();
        control.validate().unwrap();
        assert_eq!(serde_json::to_string(&control).unwrap(), encoded);
    }
}

#[test]
fn replay_principal_shape_is_bounded_in_persisted_envelopes() {
    for invalid in [
        "".into(),
        "a".repeat(257),
        "é".repeat(129),
        "two words".into(),
        " padded".into(),
        "trailing ".into(),
        "a\tb".into(),
        "a\nb".into(),
        "a\0b".into(),
        "a\u{00a0}b".into(),
    ] {
        let mut command = command();
        command.authorization.replay_principal = Some(invalid.clone());
        assert!(!command.authorization.valid_replay_principal());
        assert!(
            matches!(
                command.validate_integrity(),
                Err(AgentOperationError::Corrupt(_))
            ),
            "{invalid:?}"
        );
        let mut control = control();
        control.authorization.replay_principal = Some(invalid.clone());
        assert_eq!(
            control.validate(),
            Err(SandboxControlError::Corrupt),
            "{invalid:?}"
        );
    }
    for valid in ["a".repeat(256), "é".repeat(128), "principal:owner-1".into()] {
        let mut command = command();
        command.authorization.replay_principal = Some(valid.clone());
        command.validate_integrity().unwrap();
        let mut control = control();
        control.authorization.replay_principal = Some(valid);
        control.validate().unwrap();
    }
}

#[test]
fn principal_scopes_are_separate_and_independent_of_execution_authority() {
    let mut revision = authority();
    let agent = AgentId("agent".into());
    let kind = AgentOperationKind::SendMessage;
    let old_agent = revision.idempotency_scope(&agent, kind);
    let mut control = control();
    let old_control = control.scope();
    revision.replay_principal = Some("principal-a".into());
    control.authorization = revision.clone();
    let agent_scope = revision.idempotency_scope(&agent, kind);
    let control_scope = control.scope();
    assert_eq!(
        agent_scope,
        "891d7475499a5f7f0dc9b28d29a9339deed14d258313b24c2b9cca9e7cf66129"
    );
    assert_eq!(
        control_scope,
        "b744b94b77aaaf979d3b871a9a52672f77864fd08945052aeb98561b87d050b1"
    );
    assert_ne!(agent_scope, old_agent);
    assert_ne!(control_scope, old_control);
    for authority in ["authority-b", "principal-a"] {
        revision.authority_id = authority.into();
        control.authorization = revision.clone();
        assert_eq!(revision.idempotency_scope(&agent, kind), agent_scope);
        assert_eq!(control.scope(), control_scope);
        let mut legacy = revision.clone();
        legacy.replay_principal = None;
        assert_ne!(legacy.idempotency_scope(&agent, kind), agent_scope);
        let mut legacy_control = control.clone();
        legacy_control.authorization = legacy;
        assert_ne!(legacy_control.scope(), control_scope);
    }
    revision.replay_principal = Some("principal-b".into());
    control.authorization = revision.clone();
    assert_ne!(revision.idempotency_scope(&agent, kind), agent_scope);
    assert_ne!(control.scope(), control_scope);
}

#[test]
fn the_prior_strict_reader_cannot_read_the_new_authority_format() {
    #[derive(Deserialize, serde::Serialize)]
    #[serde(deny_unknown_fields)]
    struct LegacyAuthority {
        authority_id: String,
        policy_revision: u64,
        grant_revision: Option<u64>,
    }
    let old: LegacyAuthority = serde_json::from_str(LEGACY_AUTHORITY).unwrap();
    assert_eq!(serde_json::to_string(&old).unwrap(), LEGACY_AUTHORITY);
    // Readers predating the optional field cannot recover records that carry it.
    assert!(serde_json::from_str::<LegacyAuthority>(PRINCIPAL_AUTHORITY).is_err());
    let mut value: serde_json::Value = serde_json::from_str(PRINCIPAL_AUTHORITY).unwrap();
    value["unknown_field"] = serde_json::json!(true);
    assert!(serde_json::from_value::<AgentAuthorizationRevision>(value).is_err());
}

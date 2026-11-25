use super::*;
use authority::Access;
use cma_service_auth::mongodb_authority::MongoAuthorityMode;
use mongodb::bson::{Bson, Document};

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

fn status(grants: Vec<Bson>) -> Document {
    doc! {"authInfo":{"authenticatedUsers":[{"user":"fixture","db":"fixture"}],
    "authenticatedUserPrivileges":grants}}
}

#[test]
fn explicit_commands_retain_audit_identity_and_gate_cas() {
    for (flag, expected) in [
        ("--agent-log-audit-unit", AuditAction::RunUnit),
        ("--agent-log-audit-cancel", AuditAction::Cancel),
        ("--agent-log-audit-install", AuditAction::Install),
    ] {
        let Operation::Audit { action, agent, id } =
            parse::operation(&args(&[flag, "agent-original", "audit-original"])).unwrap()
        else {
            panic!("wrong operation")
        };
        assert_eq!(action, expected);
        assert_eq!(agent, "agent-original");
        assert_eq!(id, "audit-original");
    }
    let proof = parse::operation(&args(&[
        "--agent-log-active-proof-prepare",
        "agent-fixture",
    ]))
    .unwrap();
    assert!(matches!(proof, Operation::ActiveProof(id) if id == "agent-fixture"));
    let role =
        parse::operation(&args(&["--print-agent-log-operator-role", "active-proof"])).unwrap();
    assert_eq!(role.access(), Access::ActiveProof);
    let read =
        parse::operation(&args(&["--agent-log-gate-status", "agent_log_segments_v1"])).unwrap();
    assert_eq!(read.access(), Access::GateRead);
    let write = parse::operation(&args(&[
        "--agent-log-gate-close",
        "agent_log_segments_v1",
        "7",
    ]))
    .unwrap();
    assert_eq!(write.access(), Access::GateWrite);
    assert!(parse::operation(&args(&["--agent-log-gate-close", "agent_log_segments_v1"])).is_err());
    assert!(!is_command(&[]));
    assert!(!is_command(&args(&["--migration-plan"])));
}

#[test]
fn unknown_or_unbounded_arguments_fail_before_database_access() {
    for values in [
        vec!["--agent-log-unknown", "agent", "audit"],
        vec!["--agent-log-audit-unit", "agent"],
        vec!["--agent-log-audit-unit", "", "audit"],
        vec!["--agent-log-audit-install", "agent", "two ids"],
        vec!["--agent-log-audit-cancel", "agent", "audit\n"],
        vec!["--print-agent-log-operator-role", "runtime"],
    ] {
        assert!(parse::operation(&args(&values)).is_err());
    }
    assert!(parse::operation(&args(&[
        "--agent-log-audit-unit",
        &"x".repeat(513),
        "audit"
    ]))
    .is_err());
    for invalid in ["", "with space", "another.database", "a/b", "a\0b"] {
        assert!(validate_database(invalid).is_err());
    }
    assert!(validate_database("fixture").is_ok());
}

#[tokio::test]
async fn unknown_operator_prefix_never_enters_service_startup() {
    let values = args(&["--agent-log-unrecognized"]);
    assert!(is_command(&values));
    assert_eq!(
        command(&values).await.unwrap_err(),
        "unsupported Agent log operator arguments"
    );
}

#[test]
fn gate_control_requires_both_collections_in_the_verified_inventory() {
    use super::super::agent_log_segments::{writer_admission, writer_gate};
    for access in [Access::GateRead, Access::GateWrite] {
        assert!(authority::require_inventory_in(access, &[]).is_err());
        assert!(authority::require_inventory_in(access, &[writer_gate::COLLECTION]).is_err());
        assert!(authority::require_inventory_in(
            access,
            &[writer_gate::COLLECTION, writer_admission::COLLECTION]
        )
        .is_ok());
    }
    assert!(authority::require_inventory_in(Access::Audit, &[]).is_ok());
}

#[test]
fn operator_authority_accepts_exact_grants_and_refuses_missing_or_excess_grants() {
    for access in [
        Access::Audit,
        Access::ActiveProof,
        Access::GateRead,
        Access::GateWrite,
    ] {
        let grants = authority::role("fixture", access)
            .get_array("privileges")
            .unwrap()
            .clone();
        for mode in [
            MongoAuthorityMode::Isolated,
            MongoAuthorityMode::SharedSchema,
        ] {
            assert!(authority::verify(&status(grants.clone()), "fixture", mode, access).is_ok());
            let mut missing = grants.clone();
            missing.pop();
            assert!(authority::verify(&status(missing), "fixture", mode, access).is_err());
            let mut foreign = grants.clone();
            foreign.push(
                doc! {"resource":{"db":"other","collection":"agents"},"actions":["find"]}.into(),
            );
            assert!(authority::verify(&status(foreign), "fixture", mode, access).is_err());
        }
        for action in [
            "remove",
            "dropCollection",
            "collMod",
            "createIndex",
            "createCollection",
        ] {
            let mut excess = grants.clone();
            excess[0]
                .as_document_mut()
                .unwrap()
                .get_array_mut("actions")
                .unwrap()
                .push(action.into());
            assert!(authority::verify(
                &status(excess),
                "fixture",
                MongoAuthorityMode::Isolated,
                access
            )
            .is_err());
        }
    }
}

#[test]
fn read_only_gate_role_cannot_mutate_and_audit_role_cannot_rewrite_source_log() {
    let read = authority::role("fixture", Access::GateRead);
    let grants = read.get_array("privileges").unwrap().clone();
    assert!(authority::verify(
        &status(grants),
        "fixture",
        MongoAuthorityMode::Isolated,
        Access::GateWrite
    )
    .is_err());
    let audit = authority::role("fixture", Access::Audit);
    let source = audit
        .get_array("privileges")
        .unwrap()
        .iter()
        .filter_map(Bson::as_document)
        .find(|grant| {
            grant
                .get_document("resource")
                .unwrap()
                .get_str("collection")
                .ok()
                == Some("agent_log")
        })
        .unwrap();
    assert_eq!(
        source.get_array("actions").unwrap(),
        &vec![
            Bson::String("find".into()),
            Bson::String("listIndexes".into())
        ]
    );
}

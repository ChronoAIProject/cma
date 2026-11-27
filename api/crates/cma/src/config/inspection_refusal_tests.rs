use super::*;

const TARGET: &str = "sbx_00000000000000000000000000000001";
const OTHER: &str = "sbx_00000000000000000000000000000002";

fn load(
    environment: Option<&str>,
    value: Option<&str>,
) -> Result<Option<InspectionRefusalControl>, String> {
    InspectionRefusalControl::from_lookup(
        &mut |key| {
            Ok(match key {
                SETTING => value,
                "CHRONO_ENVIRONMENT" => environment,
                other => panic!("unexpected setting: {other}"),
            }
            .map(str::to_owned))
        },
        UNIX_EPOCH + Duration::from_secs(100),
    )
}

#[test]
fn inspection_control_defaults_off_and_requires_staging() {
    assert!(load(Some("staging"), None).unwrap().is_none());
    for environment in [
        None,
        Some("production"),
        Some("development"),
        Some("Staging"),
    ] {
        assert!(load(environment, Some("invalid")).unwrap().is_none());
    }
}

#[test]
fn inspection_control_selects_one_sandbox_and_one_work_step() {
    for (name, step, other_step) in [
        (
            "CloseProcess",
            AgentWorkStep::CloseProcess,
            AgentWorkStep::PublishConnection,
        ),
        (
            "PublishConnection",
            AgentWorkStep::PublishConnection,
            AgentWorkStep::CloseProcess,
        ),
    ] {
        let control = load(Some("staging"), Some(&format!("{TARGET}:{name}:200")))
            .unwrap()
            .unwrap();
        assert_eq!(
            control.expiry_for(&SandboxId(TARGET.into()), step),
            Some(UNIX_EPOCH + Duration::from_secs(200)),
        );
        assert_eq!(control.expiry_for(&SandboxId(OTHER.into()), step), None);
        assert_eq!(
            control.expiry_for(&SandboxId(TARGET.into()), other_step),
            None
        );
    }
}

#[test]
fn inspection_control_requires_a_canonical_sandbox_and_bounded_expiry() {
    for value in [
        "*:CloseProcess:200".to_owned(),
        format!("{TARGET},{OTHER}:CloseProcess:200"),
        format!("{TARGET}:OpenConversation:200"),
        format!("{TARGET}:PublishConnection"),
        format!("{TARGET}:CloseProcess:2801"),
        format!("{TARGET}:CloseProcess:200:extra"),
    ] {
        assert!(load(Some("staging"), Some(&value)).is_err());
    }
    assert!(load(
        Some("staging"),
        Some(&format!("{TARGET}:CloseProcess:2800"))
    )
    .is_ok());
    assert!(load(Some("staging"), Some(&format!("{TARGET}:CloseProcess:99"))).is_ok());
}

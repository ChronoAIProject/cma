use super::*;

fn settings(
    service: Option<&str>,
    cutover: Option<&str>,
    enabled: bool,
) -> Result<Settings, RecordError> {
    Settings::parse(|key| {
        Ok(match key {
            TRACKING => Some("true".into()),
            OWNER_KEYS => Some(enabled.to_string()),
            SERVICE_ID => service.map(str::to_owned),
            CUTOVER_ID => cutover.map(str::to_owned),
            _ => None,
        })
    })
}
#[test]
fn optional_secret_absence_is_allowed_only_with_owner_keys_disabled() {
    let valid = uuid::Uuid::from_u128(1).to_string();
    let disabled = settings(None, None, false).unwrap();
    assert!(!disabled.owner_keys);
    for value in [
        None,
        Some(""),
        Some(" "),
        Some("not-an-identifier"),
        Some("00000000-0000-0000-0000-000000000000"),
    ] {
        assert!(settings(value, Some(&valid), true).is_err());
        assert!(settings(Some(&valid), value, true).is_err());
    }
}
#[test]
fn present_secret_identifiers_preserve_exact_authority_and_errors_do_not_echo_values() {
    let service = uuid::Uuid::from_u128(1).to_string();
    let cutover = uuid::Uuid::from_u128(2).to_string();
    let parsed = settings(Some(&service), Some(&cutover), true).unwrap();
    assert_eq!(parsed.service_id.as_deref(), Some(service.as_str()));
    assert_eq!(parsed.cutover_id.as_deref(), Some(cutover.as_str()));
    let debug = format!("{parsed:?}");
    assert!(!debug.contains(&service) && !debug.contains(&cutover));
    let rejected = "identifier-canary-never-echo";
    let error = settings(Some(rejected), Some(&cutover), true).unwrap_err();
    assert!(!error.to_string().contains(rejected));
    assert!(!error.to_string().contains(&cutover));
}

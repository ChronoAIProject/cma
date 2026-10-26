use super::*;
#[test]
fn advertisement_is_enabled_by_default_and_explicitly_staged() {
    assert!(parse_api_advertisement(None).unwrap());
    assert!(parse_api_advertisement(Some("true")).unwrap());
    assert!(!parse_api_advertisement(Some("false")).unwrap());
    for value in ["", "1", "yes", "True", "FALSE", "false "] {
        assert!(parse_api_advertisement(Some(value)).is_err());
    }
}

#[test]
fn public_api_base_requires_a_clean_canonical_namespace() {
    for valid in [
        "https://bot.example/event-gateway",
        "https://gateway.example",
        "http://localhost:8090/event-gateway",
    ] {
        assert_eq!(validate_public_api_url(valid).unwrap(), valid);
    }
    for invalid in [
        "https://bot.example/event-gateway/",
        "https://bot.example/a/../event-gateway",
        "https://bot.example//event-gateway",
        "https://bot.example/%2e%2e/event-gateway",
        "https://user@bot.example/event-gateway",
        "https://bot.example/event-gateway?q=1",
        "https://bot.example/event-gateway#fragment",
        "http://bot.example/event-gateway",
    ] {
        assert!(validate_public_api_url(invalid).is_err(), "{invalid}");
    }
}

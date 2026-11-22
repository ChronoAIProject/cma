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

#[test]
fn domain_aliases_are_bounded_exact_clean_bases() {
    assert!(super::origins::parse_public_api_aliases("")
        .unwrap()
        .is_empty());
    assert_eq!(
        super::origins::parse_public_api_aliases(
            "https://old.example/event-gateway,https://new.example/event-gateway"
        )
        .unwrap()
        .len(),
        2
    );
    for invalid in [
        "https://old.example/event-gateway,",
        "https://old.example/event-gateway,*",
        "https://new.example/path?query=1",
        "http://public.example",
    ] {
        assert!(super::origins::parse_public_api_aliases(invalid).is_err());
    }
    assert!(
        super::origins::parse_public_api_aliases(&["https://old.example"; 9].join(",")).is_err()
    );
}

#[test]
fn cma_record_origin_aliases_are_bounded_exact_origins() {
    assert!(super::origins::parse_cma_record_origin_aliases("")
        .unwrap()
        .is_empty());
    assert_eq!(
        super::origins::parse_cma_record_origin_aliases(
            "http://cma-api.earlier.svc,https://cma.earlier.example"
        )
        .unwrap(),
        vec![
            "http://cma-api.earlier.svc".to_owned(),
            "https://cma.earlier.example".to_owned()
        ]
    );
    for invalid in [
        "http://cma-api.earlier.svc,",
        "http://cma-api.earlier.svc/",
        "http://cma-api.earlier.svc/api",
        "http://public.example",
        "https://user@cma.earlier.example",
        "https://cma.earlier.example?query=1",
    ] {
        assert!(
            super::origins::parse_cma_record_origin_aliases(invalid).is_err(),
            "{invalid}"
        );
    }
    assert!(super::origins::parse_cma_record_origin_aliases(
        &["https://cma.earlier.example"; 9].join(",")
    )
    .is_err());
}

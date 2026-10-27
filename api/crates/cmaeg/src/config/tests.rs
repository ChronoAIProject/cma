use super::*;

#[test]
fn duration_defaults_are_positive() {
    assert_eq!(
        positive_duration("__missing_cmaeg_key__", 2_000)
            .unwrap()
            .as_millis(),
        2_000
    );
}
#[test]
fn origins_keep_public_callbacks_strict_and_allow_configured_cluster_services() {
    for value in [
        "https://gateway.example/",
        "https://u:p@gateway.example",
        "https://gateway.example/path",
        "https://gateway.example?q=1",
        "http://public.example",
        "file:///tmp/fixture",
    ] {
        assert!(validate_origin(CMAEG_PUBLIC_URL, value).is_err());
    }
    for value in [
        "http://127.0.0.1:8090",
        "http://[::1]:8090",
        "https://gateway.example",
    ] {
        assert!(validate_origin(CMAEG_PUBLIC_URL, value).is_ok());
    }
    assert!(validate_origin(CMAEG_CMA_BASE_URL, "http://example-host-01.example-namespace.svc").is_ok());
    assert!(validate_origin(CMAEG_NYXID_BASE_URL, "http://nyxid.chrono-bot.svc").is_ok());
    assert!(validate_origin(CMAEG_PUBLIC_URL, "http://cmaeg.chrono-bot.svc").is_err());
    assert_eq!(
        validate_origin(CMAEG_CMA_BASE_URL, "https://CMA.EXAMPLE:443").unwrap(),
        "https://cma.example"
    );
}

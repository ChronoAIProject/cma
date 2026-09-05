use super::*;

#[test]
fn proxy_prefix_and_configured_schemes_are_preserved() {
    let plain = RuntimeEndpoint::parse(
        "http://provider.example/api",
        "https://runtime.example/proxy/9090",
    )
    .unwrap();
    assert_eq!(
        plain.control().unwrap().as_str(),
        "http://runtime.example/proxy/9090/runtime/control"
    );
    assert_eq!(
        plain.conversation("agent/a").unwrap().as_str(),
        "ws://runtime.example/proxy/9090/runtime/agents/agent%2Fa/conversation"
    );

    let secure = RuntimeEndpoint::parse(
        "https://provider.example",
        "runtime.example:9443/proxy/9090",
    )
    .unwrap();
    assert_eq!(
        secure.checkpoint("home-a.cma").unwrap().as_str(),
        "https://runtime.example:9443/proxy/9090/runtime/checkpoints/home-a.cma"
    );
    assert_eq!(secure.conversation("agent").unwrap().scheme(), "wss");
}

#[test]
fn endpoint_authority_and_suffix_injection_is_rejected() {
    for endpoint in [
        "ftp://runtime.example/proxy",
        "http://user@runtime.example/proxy",
        "http://runtime.example/proxy?next=other.example",
        "http://runtime.example/proxy#other",
        "http:/runtime.example/proxy",
        "https:runtime.example/proxy",
        "runtime.example:bad/proxy",
        "runtime.example/proxy\nnext",
        "//runtime.example/proxy",
        "",
    ] {
        assert!(
            RuntimeEndpoint::parse("https://provider.example", endpoint).is_err(),
            "accepted {endpoint:?}"
        );
    }
}

#[test]
fn configured_provider_scheme_is_closed() {
    assert!(RuntimeEndpoint::parse("ftp://provider.example", "runtime.example").is_err());
    assert!(RuntimeEndpoint::parse("provider.example", "runtime.example").is_err());
}

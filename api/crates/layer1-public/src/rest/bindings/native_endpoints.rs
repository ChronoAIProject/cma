//! Exact deployment authority for native tool destinations and advertisement aliases.
pub(super) fn validate_native_endpoint(endpoint: &str) -> Result<(), String> {
    let url = url::Url::parse(endpoint).map_err(|_| "invalid native tools endpoint")?;
    let loopback = url.host_str().is_some_and(|host| {
        host.trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
    });
    if endpoint.len() > 2_048
        || url.as_str() != endpoint
        || url.host_str().is_none()
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.port() == Some(0)
    {
        return Err("invalid native tools endpoint".into());
    }
    Ok(())
}

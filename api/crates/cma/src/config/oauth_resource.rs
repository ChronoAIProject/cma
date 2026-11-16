//! Preserve the configured audience bytes for exact provider-evidence matching.

pub(super) fn parse(value: Option<String>) -> Result<Option<String>, &'static str> {
    value
        .filter(|v| !v.is_empty())
        .map(|v| {
            let u = url::Url::parse(&v).map_err(|_| "Invalid OAuth API resource URL")?;
            if !(u.scheme() == "https"
                || u.scheme() == "http" && matches!(u.host_str(), Some("localhost" | "127.0.0.1")))
                || u.host_str().is_none()
                || !u.username().is_empty()
                || u.password().is_some()
                || u.query().is_some()
                || u.fragment().is_some()
            {
                return Err("Invalid OAuth API resource URL");
            }
            Ok(v)
        })
        .transpose()
}

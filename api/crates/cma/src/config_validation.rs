use super::*;

pub(super) fn strict_env(key: &str) -> Result<Option<String>, String> {
    std::env::var(key).map(Some).or_else(|error| match error {
        std::env::VarError::NotPresent => Ok(None),
        std::env::VarError::NotUnicode(_) => Err(format!("{key} is not valid Unicode")),
    })
}

pub(super) fn value_or(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
    default: &str,
) -> Result<String, String> {
    Ok(lookup(key)?.unwrap_or_else(|| default.to_owned()))
}

pub(super) fn required(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
) -> Result<String, String> {
    lookup(key)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

pub(super) fn positive_u32(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    key: &str,
    default: u32,
    minimum: u32,
) -> Result<NonZeroU32, String> {
    let value = parse_u64(
        key,
        &value_or(lookup, key, &default.to_string())?,
        u64::from(minimum),
        u64::from(u32::MAX),
    )? as u32;
    NonZeroU32::new(value).ok_or_else(|| format!("{key} must be positive"))
}

pub(super) fn parse_u64(key: &str, raw: &str, minimum: u64, maximum: u64) -> Result<u64, String> {
    let value = raw
        .parse::<u64>()
        .map_err(|_| format!("{key} must be an unsigned integer"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{key} must be between {minimum} and {maximum}"));
    }
    Ok(value)
}

pub(super) fn parse_usize(
    key: &str,
    raw: &str,
    minimum: usize,
    maximum: usize,
) -> Result<usize, String> {
    let value = raw
        .parse::<usize>()
        .map_err(|_| format!("{key} must be an unsigned integer"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{key} must be between {minimum} and {maximum}"));
    }
    Ok(value)
}

pub(super) fn parse_networks(key: &str, raw: &str) -> Result<Vec<IpNet>, String> {
    let mut networks = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .parse::<IpNet>()
                .map_err(|error| format!("{key} contains invalid network {value:?}: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    networks.sort_by_key(ToString::to_string);
    networks.dedup();
    Ok(networks)
}

pub(super) fn parse_service_ids(key: &str, raw: &str) -> Result<Vec<ServiceId>, String> {
    let values = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            if value.len() > 128
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            {
                Err(format!("{key} contains invalid service {value:?}"))
            } else {
                Ok(value.to_owned())
            }
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if values.is_empty() || values.len() > 64 {
        return Err(format!("{key} must contain between 1 and 64 services"));
    }
    Ok(values.into_iter().map(ServiceId::new).collect())
}

pub(super) fn validate_cloudflare_networks(
    gfe: &[IpNet],
    cloudflare: &[IpNet],
) -> Result<(), String> {
    if !cloudflare.is_empty() {
        let pinned = parse_networks(
            "build-pinned Cloudflare networks",
            PINNED_CLOUDFLARE_PROXY_CIDRS,
        )
        .expect("the build-pinned Cloudflare networks are valid");
        if cloudflare != pinned {
            return Err(
                "CHRONO_SERVER_CLOUDFLARE_PROXY_CIDRS must be empty or match the build-pinned set"
                    .into(),
            );
        }
    }
    if gfe.iter().any(|left| {
        cloudflare
            .iter()
            .any(|right| left.contains(&right.network()) || right.contains(&left.network()))
    }) {
        return Err("trusted proxy and Cloudflare networks must not overlap".into());
    }
    Ok(())
}

pub(super) fn validate_origin(key: &str, value: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(value).map_err(|error| format!("{key} is not a URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return Err(format!(
            "{key} must be an HTTP(S) origin without credentials or a path"
        ));
    }
    Ok(())
}

pub(super) fn parse_canonical_https_origin(key: &str, raw: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(raw)
        .map_err(|_| format!("{key} must be one canonical absolute HTTPS origin"))?;
    let canonical = parsed.origin().ascii_serialization();
    if raw.len() > 2_048
        || parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || raw != canonical
    {
        return Err(format!("{key} must be one canonical absolute HTTPS origin"));
    }
    Ok(canonical)
}

pub(super) fn validate_mongodb_uri(value: &str) -> Result<(), String> {
    if value.starts_with("mongodb://") || value.starts_with("mongodb+srv://") {
        Ok(())
    } else {
        Err("CHRONO_MONGODB_URI must use mongodb:// or mongodb+srv://".into())
    }
}

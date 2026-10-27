//! Exact Lark/Feishu resource reads through NyxID's authorized service proxy.
//! Relay reply tokens are never valid credentials for this transport.
use super::{nyxid, FetchRequest};
use crate::{
    channels::resolver::ResolvedChannel,
    media::{
        body::{read_capped, Overflow},
        FetchError,
    },
};
use cmaeg_core::{
    activity::{AttachmentSource, SourceKind},
    channel::{lark_resources::lark_resource_service, LarkResourceBinding, Secret, Source},
    seal::ReplyAddress,
};
use reqwest::{Client, Url};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{timeout_at, Instant};

const METADATA_CAP: u64 = 512 * 1024;
const DEADLINE: Duration = Duration::from_secs(30);

/// Fresh local authority precedes both new fetches and derivative cache/flight reuse.
pub(crate) async fn refresh(
    resolver: &crate::channels::resolver::ChannelResolver,
    previous: &ResolvedChannel,
    event: &cmaeg_core::seal::EventRefPayload,
    source: &SourceKind,
    locator: &AttachmentSource,
    deadline: Instant,
) -> Result<Arc<ResolvedChannel>, FetchError> {
    use crate::channels::resolver::ResolveError;
    let current = timeout_at(deadline, resolver.resolve_current(previous.id.as_str()))
        .await
        .map_err(|_| FetchError::TimedOut)?
        .map_err(|error| match error {
            ResolveError::Unavailable => FetchError::Upstream,
            ResolveError::RateLimited => FetchError::RateLimited,
            _ => FetchError::Unauthorized,
        })?;
    let old_consumer = previous
        .consumer(event.consumer.as_ref())
        .ok_or(FetchError::Unauthorized)?;
    let consumer = current
        .consumer(event.consumer.as_ref())
        .ok_or(FetchError::Unauthorized)?;
    if current.id != event.channel_id
        || current.pointer.record_id != event.record_id
        || consumer.pending
        || consumer.agent_key_hash != old_consumer.agent_key_hash
        || consumer.provider != old_consumer.provider
        || !configured(&current, source, locator)
    {
        return Err(FetchError::Unauthorized);
    }
    Ok(current)
}

/// Changes only when source authority or the exact proxy credential changes.
/// Name, profile and other sources are deliberately outside this revision.
pub(crate) fn revision(source: &Source, key: &Secret) -> Option<String> {
    if !matches!(
        source,
        Source::NyxidRelay {
            lark_resources: Some(_),
            ..
        }
    ) {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(b"cmaeg-lark-resource-v1\0");
    hash.update(serde_json::to_vec(source).ok()?);
    hash.update(Sha256::digest(key.as_str().as_bytes()));
    Some(format!("{:x}", hash.finalize()))
}

pub(crate) fn configured(
    channel: &ResolvedChannel,
    activity_source: &SourceKind,
    locator: &AttachmentSource,
) -> bool {
    let (
        SourceKind::NyxidRelay { platform, route_id },
        AttachmentSource::LarkResource {
            binding,
            bot_id,
            source_revision,
            ..
        },
    ) = (activity_source, locator)
    else {
        return false;
    };
    let Some(key) = channel.definition.secrets.channel_key.as_ref() else {
        return false;
    };
    channel.definition.sources.iter().any(|source| {
        matches!(source, Source::NyxidRelay { platform: current, route_ids, bot_id: Some(current_bot), lark_resources: Some(current_binding), .. }
            if current == platform && route_ids.contains(route_id) && current_bot == bot_id && current_binding == binding)
            && revision(source, key).as_ref() == Some(source_revision)
    })
}

pub(super) async fn response(
    http: &Client,
    base: &Url,
    request: &FetchRequest<'_>,
) -> Result<(reqwest::Response, Instant), FetchError> {
    let Some(
        locator @ AttachmentSource::LarkResource {
            binding,
            bot_id,
            message_id,
            resource_key,
            resource_kind,
            ..
        },
    ) = request.locator
    else {
        return Err(FetchError::NotRetrievable);
    };
    if !configured(request.channel, &request.activity.source, locator) {
        return Err(FetchError::Unauthorized);
    }
    let (
        SourceKind::NyxidRelay { platform, route_id },
        ReplyAddress::NyxidRelay {
            platform: reply_platform,
            route_id: reply_route,
            ..
        },
    ) = (&request.activity.source, &request.event.reply)
    else {
        return Err(FetchError::Unauthorized);
    };
    if platform != reply_platform || route_id != reply_route {
        return Err(FetchError::Unauthorized);
    }
    let key = request
        .channel
        .definition
        .secrets
        .channel_key
        .as_ref()
        .ok_or(FetchError::Unauthorized)?;
    let deadline = request.deadline.min(Instant::now() + DEADLINE);
    let (slug, origin) = lark_resource_service(*platform).ok_or(FetchError::NotRetrievable)?;
    // Independent metadata reads; no provider credential is returned or inspected.
    let (services, mut endpoints, catalog) = tokio::try_join!(
        json(
            http,
            endpoint(base, &["api", "v1", "user-services"])?,
            key,
            deadline
        ),
        json(
            http,
            endpoint(base, &["api", "v1", "endpoints"])?,
            key,
            deadline
        ),
        json(
            http,
            endpoint(
                base,
                &["api", "v1", "services", &binding.catalog_service_id]
            )?,
            key,
            deadline
        ),
    )?;
    let endpoint_id = authorize_service(binding, slug, origin, &services, &catalog)?;
    let rows = endpoints
        .get_mut("endpoints")
        .and_then(Value::as_array_mut)
        .ok_or(FetchError::Unauthorized)?;
    if !rows
        .iter()
        .any(|row| row.get("id").and_then(Value::as_str) == Some(endpoint_id))
    {
        let service = unique(&services, "services", &binding.user_service_id)?;
        if !allowed_shared_service(service)
            || endpoint_id.is_empty()
            || endpoint_id.len() > 128
            || !endpoint_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return Err(FetchError::Unauthorized);
        }
        let evidence = json(
            http,
            endpoint(
                base,
                &["api", "v1", "endpoints", endpoint_id, "authorization"],
            )?,
            key,
            deadline,
        )
        .await?;
        if evidence.get("id").and_then(Value::as_str) != Some(endpoint_id)
            || evidence.get("catalog_service_id").and_then(Value::as_str)
                != Some(&binding.catalog_service_id)
            || evidence.get("auto_connected") != Some(&Value::Bool(true))
            || evidence.get("url").is_some_and(|value| !value.is_null())
        {
            return Err(FetchError::Unauthorized);
        }
        rows.push(evidence);
    }
    authorize_metadata(binding, slug, origin, &services, &endpoints, &catalog)?;
    let identity = json(
        http,
        proxy_url(base, binding, &["open-apis", "bot", "v3", "info"], None)?,
        key,
        deadline,
    )
    .await?;
    if identity.get("code").and_then(Value::as_i64) != Some(0)
        || identity.pointer("/bot/open_id").and_then(Value::as_str) != Some(bot_id)
    {
        return Err(FetchError::Unauthorized);
    }
    let url = proxy_url(
        base,
        binding,
        &[
            "open-apis",
            "im",
            "v1",
            "messages",
            message_id,
            "resources",
            resource_key,
        ],
        Some(resource_kind.as_str()),
    )?;
    let response = send(http, url, key, deadline).await?;
    // These embedded resources are images/videos. A provider JSON error is not media.
    if response.status() != reqwest::StatusCode::OK
        || response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| {
                v.split(';')
                    .next()
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"))
            })
    {
        return Err(FetchError::Rejected);
    }
    Ok((response, deadline))
}

fn allowed_shared_service(service: &Value) -> bool {
    service
        .pointer("/credential_source/type")
        .and_then(Value::as_str)
        == Some("org")
        && service.pointer("/credential_source/allowed") == Some(&Value::Bool(true))
        && matches!(
            service
                .pointer("/credential_source/role")
                .and_then(Value::as_str),
            Some("admin" | "member")
        )
}

fn authorize_service<'a>(
    binding: &LarkResourceBinding,
    slug: &str,
    origin: &str,
    services: &'a Value,
    catalog: &Value,
) -> Result<&'a str, FetchError> {
    let service = unique(services, "services", &binding.user_service_id)?;
    let endpoint_id = service
        .get("endpoint_id")
        .and_then(Value::as_str)
        .ok_or(FetchError::Unauthorized)?;
    if service.get("slug").and_then(Value::as_str) != Some(slug)
        || service.get("catalog_service_id").and_then(Value::as_str)
            != Some(&binding.catalog_service_id)
        || service.get("is_active") != Some(&Value::Bool(true))
        || service.get("auth_method").and_then(Value::as_str) != Some("token_exchange")
        || service.get("node_id").is_some_and(|value| !value.is_null())
        || (service
            .pointer("/credential_source/type")
            .and_then(Value::as_str)
            == Some("org")
            && !allowed_shared_service(service))
        || catalog.get("id").and_then(Value::as_str) != Some(&binding.catalog_service_id)
        || catalog.get("slug").and_then(Value::as_str) != Some(slug)
        || catalog
            .get("base_url")
            .and_then(Value::as_str)
            .is_none_or(|url| url.trim_end_matches('/') != origin)
        || catalog.get("auth_method").and_then(Value::as_str) != Some("token_exchange")
        || catalog.get("is_active") != Some(&Value::Bool(true))
    {
        return Err(FetchError::Unauthorized);
    }
    Ok(endpoint_id)
}

fn authorize_metadata(
    binding: &LarkResourceBinding,
    slug: &str,
    origin: &str,
    services: &Value,
    endpoints: &Value,
    catalog: &Value,
) -> Result<(), FetchError> {
    let endpoint_id = authorize_service(binding, slug, origin, services, catalog)?;
    let selected_endpoint = unique(endpoints, "endpoints", endpoint_id)?;
    let official = selected_endpoint
        .get("url")
        .and_then(Value::as_str)
        .is_some_and(|url| url.trim_end_matches('/') == origin);
    if selected_endpoint
        .get("catalog_service_id")
        .and_then(Value::as_str)
        != Some(&binding.catalog_service_id)
        || !(official
            || selected_endpoint.get("auto_connected") == Some(&Value::Bool(true))
                && selected_endpoint.get("url").is_none_or(Value::is_null))
    {
        return Err(FetchError::Unauthorized);
    }
    Ok(())
}
fn unique<'a>(body: &'a Value, field: &str, id: &str) -> Result<&'a Value, FetchError> {
    let mut matches = body
        .get(field)
        .and_then(Value::as_array)
        .ok_or(FetchError::Unauthorized)?
        .iter()
        .filter(|item| item.get("id").and_then(Value::as_str) == Some(id));
    let row = matches.next().ok_or(FetchError::Unauthorized)?;
    if matches.next().is_some() {
        return Err(FetchError::Unauthorized);
    }
    Ok(row)
}
fn endpoint(base: &Url, path: &[&str]) -> Result<Url, FetchError> {
    let mut url = base.clone();
    url.path_segments_mut()
        .map_err(|_| FetchError::NotRetrievable)?
        .pop_if_empty()
        .extend(path);
    Ok(url)
}
fn proxy_url(
    base: &Url,
    binding: &LarkResourceBinding,
    path: &[&str],
    kind: Option<&str>,
) -> Result<Url, FetchError> {
    let mut segments = vec!["api", "v1", "proxy", binding.catalog_service_id.as_str()];
    segments.extend_from_slice(path);
    let mut url = endpoint(base, &segments)?;
    if let Some(kind) = kind {
        url.query_pairs_mut().append_pair("type", kind);
    }
    url.query_pairs_mut()
        .append_pair("_nyxid_via", &binding.user_service_id);
    Ok(url)
}
async fn send(
    http: &Client,
    url: Url,
    key: &Secret,
    deadline: Instant,
) -> Result<reqwest::Response, FetchError> {
    let response = timeout_at(
        deadline,
        http.get(url)
            .bearer_auth(key.as_str())
            .timeout(deadline.saturating_duration_since(Instant::now()))
            .send(),
    )
    .await
    .map_err(|_| FetchError::TimedOut)?
    .map_err(crate::media::body::transport)?;
    nyxid::classify(response.status())?;
    Ok(response)
}
async fn json(
    http: &Client,
    url: Url,
    key: &Secret,
    deadline: Instant,
) -> Result<Value, FetchError> {
    let body = read_capped(
        send(http, url, key, deadline).await?,
        METADATA_CAP,
        Overflow::Reject,
        deadline,
    )
    .await?;
    serde_json::from_slice(&body.bytes).map_err(|_| FetchError::Rejected)
}

#[cfg(test)]
#[path = "lark_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "lark_test_support.rs"]
mod test_support;

#[cfg(test)]
#[path = "lark_transport_tests.rs"]
mod transport_tests;

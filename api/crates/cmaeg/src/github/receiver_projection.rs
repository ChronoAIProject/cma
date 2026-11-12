use super::*;

pub(super) fn compact_projection(
    payload: &serde_json::Value,
    _app_ref: &str,
    delivery_id: &str,
    event: &str,
    action: Option<&str>,
    installation_id: u64,
) -> Result<GithubProjection, Error> {
    let object = match event {
        "issues" => payload.get("issue"),
        "pull_request" => payload.get("pull_request"),
        _ => None,
    }
    .and_then(serde_json::Value::as_object)
    .ok_or(Error::Invalid)?;
    let repository = payload
        .get("repository")
        .and_then(serde_json::Value::as_object)
        .ok_or(Error::Invalid)?;
    let sender = payload
        .get("sender")
        .and_then(serde_json::Value::as_object)
        .ok_or(Error::Invalid)?;
    let string = |value: Option<&serde_json::Value>| {
        value.and_then(serde_json::Value::as_str).map(str::to_owned)
    };
    Ok(GithubProjection {
        delivery_id: delivery_id.into(),
        event: event.into(),
        action: action.map(str::to_owned),
        installation_id,
        repository: GithubRepository {
            id: repository
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .ok_or(Error::Invalid)?,
            full_name: bounded_identifier(
                string(repository.get("full_name")).ok_or(Error::Invalid)?,
                201,
                |value| {
                    let mut parts = value.split('/');
                    parts.next().is_some_and(valid_component)
                        && parts.next().is_some_and(valid_component)
                        && parts.next().is_none()
                },
            )?,
        },
        number: object
            .get("number")
            .and_then(serde_json::Value::as_u64)
            .ok_or(Error::Invalid)?,
        html_url: github_html_url(string(object.get("html_url")).ok_or(Error::Invalid)?)?,
        sender: GithubSender {
            login: bounded_identifier(
                string(sender.get("login")).ok_or(Error::Invalid)?,
                101,
                valid_component,
            )?,
            r#type: string(sender.get("type"))
                .map(|value| {
                    bounded_identifier(value, 33, |value| {
                        value
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                    })
                })
                .transpose()?,
        },
        state: string(object.get("state"))
            .map(|value| {
                bounded_identifier(value, 33, |value| {
                    value
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
                })
            })
            .transpose()?,
        merged: object.get("merged").and_then(serde_json::Value::as_bool),
        draft: object.get("draft").and_then(serde_json::Value::as_bool),
    })
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn bounded_identifier(
    value: String,
    max_exclusive: usize,
    valid: impl FnOnce(&str) -> bool,
) -> Result<String, Error> {
    if value.is_empty() || value.len() >= max_exclusive || !valid(&value) {
        return Err(Error::Invalid);
    }
    Ok(value)
}

fn github_html_url(value: String) -> Result<String, Error> {
    let url = url::Url::parse(&value).map_err(|_| Error::Invalid)?;
    if value.len() > 2048
        || url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(Error::Invalid);
    }
    Ok(value)
}

pub(super) fn github_launch_request(
    channel: Arc<crate::channels::resolver::ResolvedChannel>,
    app_ref: &str,
    projection: &GithubProjection,
    keys: &cmaeg_core::seal::MasterKeyRing,
    now: i64,
) -> Result<crate::ingress::launch::LaunchRequest, Error> {
    let source = SourceKind::Github {
        app_ref: app_ref.to_owned(),
        installation_id: projection.installation_id,
        repository_id: projection.repository.id,
    };
    let event_id = EventId::new(projection.delivery_id.clone()).map_err(|_| Error::Invalid)?;
    let channel_key = channel.id.to_string();
    let conversation_id = ConversationId::derive(
        keys,
        &channel.id,
        channel.definition.partition.version,
        "github-channel",
    );
    let data = serde_json::to_value(projection).map_err(|_| Error::Unavailable)?;
    let ce = CloudEvent {
        specversion: "1.0".into(),
        id: projection.delivery_id.clone(),
        source: format!(
            "github://app/{}",
            match &source {
                SourceKind::Github { app_ref, .. } => app_ref,
                _ => unreachable!(),
            }
        ),
        r#type: format!("com.github.{}", projection.event),
        time: Some(
            time::OffsetDateTime::from_unix_timestamp(now)
                .map_err(|_| Error::Unavailable)?
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|_| Error::Unavailable)?,
        ),
        subject: Some(format!(
            "repository:{}/item:{}",
            projection.repository.id, projection.number
        )),
        datacontenttype: Some("application/json".into()),
        dataschema: None,
        data: Some(data),
        extensions: BTreeMap::new(),
    };
    let activity = Activity {
        thread: None,
        schema_version: 1,
        event_id: event_id.clone(),
        kind: ActivityKind::Event {
            ce,
            attachments: Vec::new(),
        },
        actor: Actor {
            id: format!("github:{}", projection.sender.login),
            display_name: None,
            kind: ActorKind::System,
        },
        conversation: ActivityConversation {
            id: channel_key,
            kind: ConversationKind::Channel,
            thread_id: None,
        },
        occurred_at: time::OffsetDateTime::from_unix_timestamp(now)
            .map_err(|_| Error::Unavailable)?
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| Error::Unavailable)?,
        source,
        routing: None,
    };
    activity.validate().map_err(|_| Error::Invalid)?;
    let event_ref = EventRef::seal(
        &EventRefPayload {
            telegram_admission: None,
            turn_trace: None,
            v: 1,
            activity_sha256: Some(activity_digest(&activity)),
            channel_id: channel.id.clone(),
            record_id: channel.pointer.record_id.clone(),
            event_id: event_id.clone(),
            partition_key: compact_partition_key("github-channel"),
            conversation_id: conversation_id.clone(),
            expires_at: now.saturating_add(7 * 86400),
            reply: ReplyAddress::None,
            consumer: None,
            lineage: None,
        },
        keys,
    )
    .map_err(|_| Error::Unavailable)?;
    Ok(crate::ingress::launch::LaunchRequest {
        channel,
        activity,
        conversation_id,
        event_ref,
        idempotency_key: IdempotencyKey::from_event(&event_id),
        received_at: Instant::now(),
        trace: tracing::Span::current(),
        timing: None,
        turn_trace: None,
        consumer: None,
    })
}

#[cfg(test)]
mod tests {
    use super::super::backoff;
    use serde_json::json;

    #[test]
    fn projection_drops_issue_title_and_body_before_template_rendering() {
        let payload = json!({
            "issue": {"id": 9, "number": 4, "html_url": "https://github.com/org/repo/issues/4", "state": "open", "title": "title-sentinel", "body": "body-sentinel"},
            "repository": {"id": 42, "full_name": "org/repo"},
            "sender": {"login": "octocat", "type": "User"}
        });
        let projection =
            super::compact_projection(&payload, "app", "delivery", "issues", Some("opened"), 7)
                .unwrap();
        let encoded = serde_json::to_string(&projection).unwrap();
        assert!(!encoded.contains("title-sentinel"));
        assert!(!encoded.contains("body-sentinel"));
        assert!(encoded.contains("org/repo"));
    }

    #[test]
    fn dispatch_backoff_is_bounded() {
        assert_eq!(backoff(0), 5);
        assert_eq!(backoff(8), 1280);
        assert_eq!(backoff(99), 1280);
    }

    #[test]
    fn projection_rejects_non_github_urls_and_unbounded_identifiers() {
        let mut payload = json!({
            "issue": {"id": 9, "number": 4, "html_url": "https://example.test/i/4", "state": "open"},
            "repository": {"id": 42, "full_name": "org/repo"},
            "sender": {"login": "octocat", "type": "User"}
        });
        assert!(super::compact_projection(
            &payload,
            "app",
            "delivery",
            "issues",
            Some("opened"),
            7
        )
        .is_err());
        payload["issue"]["html_url"] = "https://github.com/org/repo/issues/4".into();
        payload["sender"]["login"] = "!invalid".into();
        assert!(super::compact_projection(
            &payload,
            "app",
            "delivery",
            "issues",
            Some("opened"),
            7
        )
        .is_err());
    }
}

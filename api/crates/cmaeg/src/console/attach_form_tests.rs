use super::*;

fn form(fields: &[(&str, &str)]) -> Vec<u8> {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(fields)
        .finish()
        .into_bytes()
}

#[test]
fn a_telegram_form_becomes_a_telegram_source_with_its_secrets() {
    let parsed = parse(&form(&[
        ("expected_version", "4"),
        ("type", "telegram"),
        ("bot_id", " 7000000001 "),
        ("bot_username", "fixture_bot"),
        ("admission", "scoped"),
        ("senders", "allowlist"),
        ("sender_ids", "a, b\nc"),
        ("chats", "open"),
        ("groups", "mention"),
        ("telegram_bot_token", "token-value"),
        ("telegram_webhook_secret", ""),
    ]))
    .unwrap();
    assert_eq!(parsed.request.expected_version, 4);
    assert!(parsed.generated.is_none());
    let SourceRequest::Telegram {
        topic_lifecycle: _,
        topic_creation: _,
        message_events: _,
        channel_posts: _,
        member_join_chat_ids: _,
        bot_id,
        bot_username,
        admission,
    } = parsed.request.source
    else {
        panic!("not telegram");
    };
    assert_eq!(bot_id, "7000000001");
    assert_eq!(bot_username.as_deref(), Some("fixture_bot"));
    assert_eq!(
        admission,
        AdmissionPolicy::Scoped {
            bot_messages: Default::default(),
            senders: IdentityAdmission::Allowlist(vec!["a".into(), "b".into(), "c".into()]),
            chats: IdentityAdmission::Open,
            groups: GroupAdmission::Mention,
        }
    );
    let credentials = parsed.request.credentials;
    assert_eq!(
        credentials.telegram_bot_token.as_ref().map(Secret::as_str),
        Some("token-value")
    );
    assert!(credentials.telegram_webhook_secret.is_none());
}

#[test]
fn a_relay_form_names_its_platform_and_routes() {
    let parsed = parse(&form(&[
        ("expected_version", "1"),
        ("type", "nyxid_relay"),
        ("issuer", "https://nyxid.example"),
        ("key_id", "relay-key"),
        ("route_ids", "r1,r2"),
        ("platform", "lark"),
        ("trust_normalized", "on"),
        ("admission", "allowlist"),
        ("sender_ids", "ou_1"),
        ("channel_key", "shared-key"),
    ]))
    .unwrap();
    let SourceRequest::NyxidRelay {
        route_ids,
        platform,
        trust_normalized,
        admission,
        bot_id,
        ..
    } = parsed.request.source
    else {
        panic!("not a relay");
    };
    assert_eq!(route_ids, ["r1", "r2"]);
    assert_eq!(platform, Platform::Lark);
    assert!(trust_normalized);
    assert!(bot_id.is_none());
    assert_eq!(
        admission,
        AdmissionPolicy::Allowlist {
            sender_ids: vec!["ou_1".into()]
        }
    );
    assert!(parsed.request.credentials.channel_key.is_some());
}

#[test]
fn a_custom_form_can_ask_the_gateway_to_generate_its_secret() {
    let parsed = parse(&form(&[
        ("expected_version", "2"),
        ("type", "custom"),
        ("auth", "hmac"),
        ("key_id", "k1"),
        ("hmac_secret", "typed-but-ignored"),
        ("generate_secret", "on"),
        ("schema", "{\"type\":\"object\"}"),
        ("attachments_pointer", "/files"),
        (
            "attachments_origins",
            "https://a.example\nhttps://b.example",
        ),
        ("partition_custom_key", "event_subject"),
    ]))
    .unwrap();
    let generated = parsed.generated.expect("a generated secret");
    assert_eq!(generated.key_id, "k1");
    assert_eq!(generated.secret.as_str().len(), 44);
    let secrets = &parsed.request.credentials.hmac_secrets;
    assert_eq!(secrets.len(), 1);
    assert_eq!(secrets[0].secret, generated.secret);
    assert_eq!(
        parsed.request.partition_custom_key,
        Some(PartitionKey::EventSubject)
    );
    let SourceRequest::Custom {
        schema,
        attachments,
        ..
    } = parsed.request.source
    else {
        panic!("not custom");
    };
    assert!(schema.is_some());
    assert_eq!(attachments.unwrap().origins.len(), 2);
    // Two generations never repeat.
    let again = parse(&form(&[
        ("expected_version", "2"),
        ("type", "custom"),
        ("auth", "hmac"),
        ("key_id", "k1"),
        ("generate_secret", "on"),
    ]))
    .unwrap();
    assert_ne!(again.generated.unwrap().secret, generated.secret);
}

#[test]
fn malformed_forms_are_refused_with_fixed_text() {
    for (fields, expected) in [
        (
            vec![("expected_version", "x"), ("type", "telegram")],
            "out of date",
        ),
        (
            vec![("expected_version", "1"), ("type", "smtp")],
            "source type",
        ),
        (
            vec![("expected_version", "1"), ("type", "telegram")],
            "bot id",
        ),
        (
            vec![
                ("expected_version", "1"),
                ("type", "custom"),
                ("auth", "nyxid_key"),
                ("schema", "{not json"),
            ],
            "must be JSON",
        ),
        (
            vec![
                ("expected_version", "1"),
                ("type", "nyxid_relay"),
                ("issuer", "i"),
                ("key_id", "k"),
                ("platform", "<script>"),
            ],
            "platform",
        ),
        (
            vec![("expected_version", "1"), ("expected_version", "2")],
            "could not be read",
        ),
    ] {
        let error = parse(&form(&fields)).err().expect("refused");
        assert!(error.contains(expected), "{fields:?}: {error}");
    }
}

#[test]
fn a_kept_submission_never_holds_a_secret_or_the_csrf_value() {
    let kept = Submitted::of(&form(&[
        ("csrf", "csrf-value"),
        ("type", "telegram"),
        ("bot_id", "7000000001"),
        ("telegram_bot_token", "token-value"),
        ("telegram_webhook_secret", "webhook-value"),
        ("channel_key", "channel-value"),
        ("hmac_secret", "hmac-value"),
    ]));
    assert_eq!(kept.kind(), "telegram");
    assert_eq!(kept.value("bot_id"), "7000000001");
    for field in ["csrf", "telegram_bot_token", "telegram_webhook_secret"] {
        assert_eq!(kept.value(field), "", "{field}");
    }
    assert_eq!(kept.value("channel_key"), "");
    assert_eq!(kept.value("hmac_secret"), "");
}

use super::*;

fn choices() -> ConnectionChoices {
    ConnectionChoices {
        form_action: "/connect/action".into(),
        csrf_token: "browser-bound-fixture-csrf".into(),
        selected: ConnectionMethod::Existing,
        managers: vec![ManagerChoice {
            connection_ref: "connection-fixture-opaque".into(),
            label: "Our creation bot".into(),
        }],
        management: Some(ManagementChoice {
            instructions: "Enable Bot Management Mode in the official BotFather Mini App. The gateway will check that it is enabled.".into(),
            enable_link: PageLink {
                label: "Open BotFather".into(),
                href: "https://t.me/BotFather".into(),
            },
        }),
        registration_help: Some(PageLink {
            label: "Find your bot token".into(),
            href: "https://core.telegram.org/bots/features#botfather".into(),
        }),
        error: None,
    }
}

fn page(state: ConnectionSetupState) -> ConnectionSetupPage {
    ConnectionSetupPage {
        owner_label: "Example organization".into(),
        platform_label: "Telegram".into(),
        state,
    }
}

#[test]
fn existing_and_new_bots_submit_only_explicit_browser_forms() {
    let html = render_document(&page(ConnectionSetupState::Choose(choices()))).unwrap();
    assert_eq!(html.matches("method=\"post\"").count(), 2);
    assert_eq!(html.matches("action=\"/connect/action\"").count(), 2);
    assert_eq!(html.matches("name=\"csrf_token\"").count(), 2);
    assert!(html.contains("name=\"intent\" value=\"connect_existing\""));
    assert!(html.contains("name=\"intent\" value=\"create_managed\""));
    assert!(html.contains("type=\"password\" name=\"bot_token\""));
    assert!(!html.contains("name=\"bot_token\" value="));
    assert!(html.contains("name=\"manager_connection_ref\""));
    assert!(html.contains("value=\"connection-fixture-opaque\""));
    assert_eq!(html.matches("name=\"request_management\"").count(), 2);
    assert!(html.contains("Your bot can still have normal conversations."));
    for forbidden in [
        "<script",
        "localStorage",
        "sessionStorage",
        "name=\"bearer\"",
        "name=\"access_token\"",
        "name=\"owner\"",
    ] {
        assert!(
            !html.contains(forbidden),
            "unexpected browser authority field: {forbidden}"
        );
    }
}

#[test]
fn all_dynamic_text_and_attribute_values_are_escaped() {
    let attack = "\"'><img src=x onerror=alert(1)> & injected";
    let mut choice = choices();
    choice.managers[0].label = attack.into();
    choice.managers[0].connection_ref = attack.into();
    choice.csrf_token = attack.into();
    choice.error = Some(attack.into());
    choice.selected = ConnectionMethod::Create;
    choice.management.as_mut().unwrap().instructions = attack.into();
    choice.registration_help.as_mut().unwrap().label = attack.into();
    let mut input = page(ConnectionSetupState::Choose(choice));
    input.owner_label = attack.into();
    input.platform_label = attack.into();
    let html = render_document(&input).unwrap();
    assert!(!html.contains("<img"));
    assert!(!html.contains(attack));
    assert!(html.contains("&quot;&#39;&gt;&lt;img src=x onerror=alert(1)&gt; &amp; injected"));
    assert!(html.contains("value=\"create\" checked"));
    assert!(!html.contains("value=\"existing\" checked"));
}

#[test]
fn actions_cannot_move_credentials_to_another_origin_or_query_string() {
    for action in [
        "https://attacker.example/collect",
        "//attacker.example/collect",
        "/\\attacker.example/collect",
        "javascript:alert(1)",
        "connect/action",
        "/connect?access_token=fixture",
        "/connect#fragment",
        "/connect\n/action",
    ] {
        let mut choice = choices();
        choice.form_action = action.into();
        assert_eq!(
            render_document(&page(ConnectionSetupState::Choose(choice))).unwrap_err(),
            RenderError::InvalidAction,
            "unsafe form target {action:?}",
        );
    }
    let mut choice = choices();
    choice.csrf_token = "   ".into();
    assert_eq!(
        render_document(&page(ConnectionSetupState::Choose(choice))).unwrap_err(),
        RenderError::MissingCsrf,
    );
}

#[test]
fn only_official_https_platform_links_and_root_paths_are_accepted() {
    for href in [
        "https://t.me/BotFather",
        "https://t.me/newbot/ExampleManager/paper_bot?name=Paper+assistant",
        "https://core.telegram.org/bots/features#managed-bots",
        "/connect",
    ] {
        assert!(allowed_link(href), "expected safe link {href}");
    }
    for href in [
        "javascript:alert(1)",
        "data:text/html,hello",
        "http://t.me/BotFather",
        "https://t.me.evil.example/BotFather",
        "https://t.me@evil.example/BotFather",
        "https://user:password@t.me/BotFather",
        "https://t.me:8443/BotFather",
        "https://other.example/connect",
        "//t.me/BotFather",
        "/\\evil.example/collect",
        "https://t.me/\nBotFather",
    ] {
        assert!(!allowed_link(href), "unexpected accepted link {href:?}");
        let input = page(ConnectionSetupState::Complete {
            connection_label: "Example bot".into(),
            action: Some(PageLink {
                label: "Return".into(),
                href: href.into(),
            }),
        });
        assert_eq!(
            render_document(&input).unwrap_err(),
            RenderError::InvalidLink
        );
    }
}

#[test]
fn unavailable_managers_cannot_submit_managed_creation() {
    let mut choice = choices();
    choice.managers.clear();
    choice.selected = ConnectionMethod::Create;
    let html = render_document(&page(ConnectionSetupState::Choose(choice))).unwrap();
    assert!(html.contains("No creation bot is available yet"));
    assert!(!html.contains("name=\"intent\" value=\"create_managed\""));
    assert!(html.contains("name=\"intent\" value=\"connect_existing\""));
}

#[test]
fn server_state_controls_completion_and_pending_pages_have_no_creation_side_effects() {
    let input = page(ConnectionSetupState::AwaitingPlatform {
        message: "Confirm the new bot in Telegram.".into(),
        action: Some(PageLink {
            label: "Open Telegram".into(),
            href: "https://t.me/newbot/ExampleManager/paper_bot?name=Paper+assistant".into(),
        }),
        refresh_path: "/connect".into(),
        retry: None,
    });
    let html = render_document(&input).unwrap();
    assert!(html.contains("Waiting for you"));
    assert!(html.contains("href=\"/connect\">Check connection"));
    assert!(!html.contains("Your bot is connected"));
    assert!(!html.contains("<form"));
    assert!(!html.contains("http-equiv=\"refresh\""));
    let complete = render_document(&page(ConnectionSetupState::Complete {
        connection_label: "Paper assistant".into(),
        action: None,
    }))
    .unwrap();
    assert!(complete.contains("Your bot is connected"));
    assert!(complete.contains("Paper assistant"));
    assert!(!complete.contains("<form"));
}

#[test]
fn management_is_optional_and_never_an_exclusive_bot_type() {
    let mut choice = choices();
    choice.management = None;
    let html = render_document(&page(ConnectionSetupState::Choose(choice))).unwrap();
    assert!(!html.contains("name=\"request_management\""));
    assert!(html.contains("Connect an existing bot"));
    assert!(html.contains("Create a new bot"));
}

#[test]
fn pending_retry_uses_only_the_existing_browser_operation_and_same_origin_csrf_post() {
    let input = page(ConnectionSetupState::AwaitingPlatform {
        message: "Use the creation button in your conversation.".into(),
        action: None,
        refresh_path: "/connect".into(),
        retry: Some(ConnectionRetry {
            form_action: "/connect/submit".into(),
            csrf_token: "browser-csrf\"'><unsafe>".into(),
        }),
    });
    let html = render_document(&input).unwrap();
    assert_eq!(html.matches("<form").count(), 1);
    assert!(html.contains("method=\"post\" action=\"/connect/submit\""));
    assert!(html.contains("name=\"intent\" value=\"retry_creation\""));
    assert!(
        html.contains("name=\"csrf_token\" value=\"browser-csrf&quot;&#39;&gt;&lt;unsafe&gt;\"")
    );
    assert!(html.contains("Resend creation button"));
    assert!(html.contains("Resend only if the creation button is missing"));
    for field in [
        "create_managed",
        "connect_existing",
        "name=\"bot_token\"",
        "name=\"record_id\"",
    ] {
        assert!(!html.contains(field));
    }
    assert!(!html.contains("<unsafe>"));
    for (action, csrf, expected) in [
        (
            "https://foreign.example/collect",
            "csrf",
            RenderError::InvalidAction,
        ),
        (
            "//foreign.example/collect",
            "csrf",
            RenderError::InvalidAction,
        ),
        ("/connect?csrf=leaked", "csrf", RenderError::InvalidAction),
        ("/connect/submit", "   ", RenderError::MissingCsrf),
    ] {
        let input = page(ConnectionSetupState::AwaitingPlatform {
            message: "Pending".into(),
            action: None,
            refresh_path: "/connect".into(),
            retry: Some(ConnectionRetry {
                form_action: action.into(),
                csrf_token: csrf.into(),
            }),
        });
        assert_eq!(render_document(&input).unwrap_err(), expected);
    }
}

#[tokio::test]
async fn private_views_preserve_post_origin_without_cross_origin_referrer_leaks() {
    let response = render(&page(ConnectionSetupState::Choose(choices()))).unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    // no-referrer makes browser form POSTs use Origin:null, breaking the
    // gateway's same-origin CSRF check. External links still receive no referrer.
    assert_eq!(response.headers()[header::REFERRER_POLICY], "same-origin");
    assert_eq!(response.headers()[header::X_FRAME_OPTIONS], "DENY");
    let policy = response.headers()[header::CONTENT_SECURITY_POLICY]
        .to_str()
        .unwrap();
    assert!(policy.contains("default-src 'none'"));
    assert!(policy.contains("form-action 'self'"));
    assert!(!policy.contains("unsafe-inline"));
    assert!(policy.contains(&STANDARD.encode(Sha256::digest(STYLE.as_bytes()))));
    let body = axum::body::to_bytes(response.into_body(), 128 * 1024)
        .await
        .unwrap();
    assert!(std::str::from_utf8(&body).unwrap().contains(STYLE));
    let expired = render(&page(ConnectionSetupState::Expired { restart: None })).unwrap();
    assert_eq!(expired.status(), StatusCode::GONE);
    let html = axum::body::to_bytes(expired.into_body(), 128 * 1024)
        .await
        .unwrap();
    assert!(!std::str::from_utf8(&html).unwrap().contains("<form"));
    assert!(std::str::from_utf8(&html)
        .unwrap()
        .contains("Return to the assistant where you started"));
}

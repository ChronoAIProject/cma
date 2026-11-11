use super::*;

#[test]
fn canonical_title_is_one_validated_value_for_every_source() {
    let title = AgentTitle::new("Durable Agent Titles").unwrap();
    assert_eq!(title.as_str(), "Durable Agent Titles");
    assert_eq!(
        serde_json::to_string(&title).unwrap(),
        "\"Durable Agent Titles\""
    );
    assert_eq!(
        serde_json::from_str::<AgentTitle>("\"Durable Agent Titles\"").unwrap(),
        title
    );

    assert_eq!(AgentTitle::new(""), Err(AgentTitleError::Empty));
    assert_eq!(
        AgentTitle::new(" padded "),
        Err(AgentTitleError::NotSanitized)
    );
    assert_eq!(
        AgentTitle::new("control\u{0000}title"),
        Err(AgentTitleError::ContainsControl)
    );
    assert_eq!(
        AgentTitle::new("x".repeat(AGENT_TITLE_MAX_SCALARS + 1)),
        Err(AgentTitleError::TooLong)
    );
    for unsanitized in ["\"quoted\"", "**ornament**", "### heading", "[boxed]"] {
        assert_eq!(
            AgentTitle::new(unsanitized),
            Err(AgentTitleError::NotSanitized)
        );
    }
    assert!(serde_json::from_str::<AgentTitle>("\" padded \"").is_err());
}

#[test]
fn sanitizer_keeps_only_a_control_free_unornamented_first_line() {
    let raw = concat!(
        "  ### **\u{201c}Durable\tAgent Titles\u{201d}** ###  ",
        "\nprivate second-line marker"
    );
    let title = AgentTitle::sanitize(raw).unwrap();
    assert_eq!(title.as_str(), "DurableAgent Titles");
    assert!(!title.as_str().contains("private"));
    assert!(!title.as_str().chars().any(char::is_control));

    for (raw, expected) in [
        ("'ASCII Quotes'", "ASCII Quotes"),
        ("`Code Ornament`", "Code Ornament"),
        ("- List Ornament", "List Ornament"),
        ("\u{2022} Bullet Ornament \u{2728}", "Bullet Ornament"),
        ("\u{00ab}Unicode Quotes\u{00bb}", "Unicode Quotes"),
        ("【Bracket Ornament】", "Bracket Ornament"),
    ] {
        assert_eq!(AgentTitle::sanitize(raw).unwrap().as_str(), expected);
    }
    for empty in ["\r\nsecret", "\t\0", "***", "[]", "{}", "\u{2728}\u{2728}"] {
        assert!(AgentTitle::sanitize(empty).is_none());
    }
}

#[test]
fn generated_title_is_bounded_by_unicode_scalars_not_bytes() {
    let raw = format!("{}ignored", "\u{754c}".repeat(AGENT_TITLE_MAX_SCALARS));
    let title = AgentTitle::sanitize(&raw).unwrap();
    assert_eq!(title.as_str(), "\u{754c}".repeat(AGENT_TITLE_MAX_SCALARS));
    assert_eq!(title.as_str().chars().count(), AGENT_TITLE_MAX_SCALARS);
    assert!(title.as_str().len() > AGENT_TITLE_MAX_SCALARS);
}

#[test]
fn fallback_uses_48_sanitized_owner_scalars_or_the_static_default() {
    let owner = format!("  \"{}tail\"\nprivate", "\u{754c}".repeat(60));
    let fallback = AgentTitle::fallback(Some(&owner));
    assert_eq!(
        fallback.as_str(),
        "\u{754c}".repeat(AGENT_TITLE_FALLBACK_SCALARS)
    );
    assert_eq!(
        fallback.as_str().chars().count(),
        AGENT_TITLE_FALLBACK_SCALARS
    );
    for owner in [None, Some("\nsecret"), Some("***"), Some("\0\t")] {
        assert_eq!(
            AgentTitle::fallback(owner).as_str(),
            EMPTY_AGENT_TITLE_FALLBACK
        );
    }
}

#[test]
fn transcript_and_request_bound_each_utf8_input_independently() {
    let owner = format!(
        "{}\u{754c}owner-tail",
        "o".repeat(AGENT_TITLE_INPUT_BYTES - 1)
    );
    let assistant = format!(
        "{}\u{754c}assistant-tail",
        "a".repeat(AGENT_TITLE_INPUT_BYTES - 2)
    );
    let transcript = AgentTitleTranscript::completed(1, &owner, &assistant);
    assert!(transcript.owner_message().len() <= AGENT_TITLE_INPUT_BYTES);
    assert!(transcript.assistant_message().len() <= AGENT_TITLE_INPUT_BYTES);
    assert!(transcript
        .owner_message()
        .is_char_boundary(transcript.owner_message().len()));
    assert!(transcript
        .assistant_message()
        .is_char_boundary(transcript.assistant_message().len()));
    assert!(!transcript.owner_message().contains("owner-tail"));
    assert!(!transcript.assistant_message().contains("assistant-tail"));

    let context = AgentTitleGenerationContext::new("pinned-model", "private-bearer").unwrap();
    let request = AgentTitleGenerationRequest::new(&context, &transcript);
    assert_eq!(request.model(), "pinned-model");
    assert_eq!(request.system_prompt(), AGENT_TITLE_SYSTEM_PROMPT);
    assert_eq!(request.owner_message(), transcript.owner_message());
    assert_eq!(request.assistant_message(), transcript.assistant_message());
    assert_eq!(request.bearer(), "private-bearer");
}

#[test]
fn every_debuggable_private_boundary_redacts_content() {
    let owner = "owner-private-marker";
    let assistant = "assistant-private-marker";
    let bearer = "bearer-private-marker";
    let generated = "generated-private-marker";
    let context = AgentTitleGenerationContext::new("visible-model", bearer).unwrap();
    let transcript = AgentTitleTranscript::completed(1, owner, assistant);
    let request = AgentTitleGenerationRequest::new(&context, &transcript);
    let response = AgentTitleGenerationResponse::new(generated);
    let title = AgentTitle::new(generated).unwrap();
    let debug = format!(
        "{context:?} {transcript:?} {request:?} {response:?} {title:?} {:?}",
        AgentTitleGenerationError::unavailable()
    );
    for private in [owner, assistant, bearer, generated] {
        assert!(!debug.contains(private));
    }
    assert!(debug.contains("visible-model"));
}

#[test]
fn injection_shaped_text_stays_one_inert_title_value() {
    let injection = r#"Safe"},"status":"terminated","owner":"other"#;
    let title = AgentTitle::sanitize(injection).unwrap();
    let encoded = serde_json::to_value(&title).unwrap();
    assert_eq!(encoded, injection);
    assert_eq!(title.as_str(), injection);
}

#[test]
fn generation_constants_preserve_the_bounded_protocol() {
    assert!(AGENT_TITLE_SYSTEM_PROMPT.contains("3-6 word"));
    assert_eq!(AGENT_TITLE_INPUT_BYTES, 2 * 1024);
    assert_eq!(AGENT_TITLE_MAX_SCALARS, 64);
    assert_eq!(AGENT_TITLE_FALLBACK_SCALARS, 48);
    assert_eq!(AGENT_TITLE_CALL_TIMEOUT_SECONDS, 10);
    assert_eq!(AGENT_TITLE_CLAIM_SECONDS, 45);
    assert_eq!(AGENT_TITLE_ATTEMPTS, 1);
    assert_eq!(AGENT_TITLE_MAX_ATTEMPTS, 3);
    assert_eq!(AGENT_TITLE_RETRY_SECONDS, 60);
    assert!(AgentTitleGenerationContext::new("", "bearer").is_none());
    assert!(AgentTitleGenerationContext::new("model", "").is_none());
}

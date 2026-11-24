use super::*;
use std::{collections::BTreeMap, process::Command};

/// Gates left default-off: each needs a per-environment companion setting,
/// an operator-run migration or an explicit operator attestation.
const STILL_EXPLICIT: &[&str] = &[
    "CMAEG_BINDING_PREPARATION_ENABLED",
    "CMAEG_CHANNEL_MUTATION_CUSTODY_ENABLED",
    "CMAEG_CONSOLE_ENABLED",
    "CMAEG_CONSOLE_SHARED_SESSIONS",
    "CMAEG_CONSUMER_INTERACTIONS_ENABLED",
    "CMAEG_LARK_CARDS_ENABLED",
    "CMAEG_MANAGED_TELEGRAM_SETUP_ENABLED",
    "CMAEG_MEDIA_CONTINUATIONS_ENABLED",
    "CMAEG_MEDIA_ENABLED",
    "CMAEG_NATIVE_DISCORD_VOICE_ENABLED",
    "CMAEG_NYXID_COMMONMARK_LARK_FEISHU_DISCORD_ENABLED",
    "CMAEG_NYXID_COMMONMARK_RELAY_ENABLED",
    "CMAEG_NYXID_COMPLETE_REPLIES",
    "CMAEG_NYXID_NATIVE_MEDIA_ENABLED",
    "CMAEG_OWNER_KEYS_ENABLED",
    "CMAEG_RELAY_TEXT_EDITS_ENABLED",
    "CMAEG_REPLICAS_ENABLED",
    "CMAEG_TELEGRAM_MESSAGE_EFFECTS_ENABLED",
    "CMAEG_WHATSAPP_INTERACTIONS_ENABLED",
    "CMAEG_WHATSAPP_TEMPLATES_ENABLED",
    "CMAEG_WORKER_CREDENTIAL_TRACKING_ENABLED",
];

fn read(name: &str, env: &[(&str, &str)]) -> Result<bool, ConfigError> {
    flag(name, &mut |key| {
        env.iter()
            .find(|(candidate, _)| *candidate == key)
            .map(|(_, value)| (*value).to_owned())
            .ok_or(VarError::NotPresent)
    })
}

fn names() -> impl Iterator<Item = &'static str> {
    GRADUATED.iter().map(|(name, _)| *name)
}

/// Whether `name` is `off` or transitively requires it.
fn depends_on(name: &str, off: &str) -> bool {
    name == off
        || prerequisites(name)
            .unwrap_or_default()
            .iter()
            .any(|prerequisite| depends_on(prerequisite, off))
}

#[test]
fn every_graduated_gate_is_on_when_unset_and_off_when_explicitly_false() {
    assert_eq!(GRADUATED.len(), 82);
    for name in names() {
        assert!(read(name, &[]).unwrap(), "{name} unset");
        assert!(read(name, &[(name, "true")]).unwrap(), "{name}=true");
        assert!(!read(name, &[(name, "false")]).unwrap(), "{name}=false");
        for value in ["", "TRUE", "1", " true ", "yes"] {
            assert_eq!(
                read(name, &[(name, value)]).unwrap_err().to_string(),
                format!("management configuration is invalid: {name} must be true or false"),
            );
        }
    }
}

#[test]
fn the_graduated_list_is_sorted_unique_and_its_prerequisites_are_graduated() {
    let listed: Vec<_> = names().collect();
    let mut sorted = listed.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(listed, sorted);
    for (name, requires) in GRADUATED {
        for prerequisite in *requires {
            assert_ne!(name, prerequisite);
            assert!(prerequisites(prerequisite).is_some(), "{prerequisite}");
            assert!(!depends_on(prerequisite, name), "{name} is cyclic");
        }
    }
    for name in STILL_EXPLICIT {
        assert!(prerequisites(name).is_none(), "{name} must stay explicit");
        assert!(!read(name, &[]).unwrap(), "{name} unset");
        assert!(read(name, &[(name, "true")]).unwrap(), "{name}=true");
    }
}

#[test]
fn switching_a_prerequisite_off_turns_off_only_its_unset_dependents() {
    for off in [CMAEG_ROUTED_RECOVERY_ENABLED, CMAEG_NATIVE_DISCORD_ENABLED] {
        let env = [(off, "false")];
        for name in names() {
            assert_eq!(read(name, &env).unwrap(), !depends_on(name, off), "{name}");
            // An explicit value is still honoured as given.
            let explicit = [(off, "false"), (name, "true")];
            assert_eq!(read(name, &explicit).unwrap(), name != off, "{name}");
        }
    }
    assert!(!read(
        "CMAEG_ROUTED_ADMISSION_ENABLED",
        &[(CMAEG_ROUTED_RECOVERY_ENABLED, "false")]
    )
    .unwrap());
    assert!(!read(
        "CMAEG_NATIVE_DISCORD_FORWARDS_ENABLED",
        &[(CMAEG_NATIVE_DISCORD_ENABLED, "false")]
    )
    .unwrap());
    // An invalid prerequisite keeps its own error.
    assert_eq!(
        read(
            "CMAEG_ROUTED_ADMISSION_ENABLED",
            &[(CMAEG_ROUTED_RECOVERY_ENABLED, "on")]
        )
        .unwrap_err()
        .to_string(),
        "management configuration is invalid: CMAEG_ROUTED_RECOVERY_ENABLED must be true or false"
    );
}

/// The staging Event Gateway's deployed literal environment.
const STAGING: &[(&str, &str)] = &[
    ("CMAEG_CHANNEL_MUTATION_CUSTODY_ENABLED", "false"),
    (
        "CMAEG_CMA_BASE_URL",
        "http://cma-agent-manager.chrono-ai-staging.svc:8080",
    ),
    ("CMAEG_CONSOLE_ENABLED", "false"),
    ("CMAEG_CONSOLE_PUBLIC_URL", "https://cma-staging.nyxid.ai"),
    ("CMAEG_LISTEN", "0.0.0.0:8090"),
    ("CMAEG_MEDIA_ENABLED", "false"),
    ("CMAEG_MEDIA_FFMPEG_DIR", ""),
    ("CMAEG_METRICS_LISTEN", "0.0.0.0:9091"),
    ("CMAEG_MONGODB_AUTHORITY_MODE", "shared-schema"),
    ("CMAEG_MONGODB_DATABASE", "cma"),
    ("CMAEG_NYXID_BASE_URL", "https://staging-api.nyxid.ai"),
    (
        "CMAEG_PUBLIC_API_URL",
        "https://cma-staging.nyxid.ai/event-gateway",
    ),
    ("CMAEG_PUBLIC_URL", "https://cmaeg-staging.nyxid.ai"),
    ("CMAEG_REPLICAS_ENABLED", "false"),
    ("CMAEG_TOOLS_NYXID_SLUG", "cma_staging_eg"),
    ("CMAEG_WEB_ORIGIN", "https://cma-staging.nyxid.ai"),
    ("RUST_MIN_STACK", "16777216"),
];

/// The production Event Gateway's literal environment without the graduated
/// gates. Identifiers are synthetic.
const PRODUCTION: &[(&str, &str)] = &[
    ("CMAEG_ACCEPT_BUDGET_MS", "20000"),
    ("CMAEG_ACK_TEXT", "Working on it…"),
    ("CMAEG_ADVERTISE_PUBLIC_API_URL", "true"),
    ("CMAEG_BINDING_PREPARATION_ENABLED", "true"),
    ("CMAEG_BINDING_REPLACEMENT_BINDINGS", ""),
    ("CMAEG_BUSY_RETRY_MAX_MS", "180000"),
    ("CMAEG_CHANNEL_CACHE_MAX", "50000"),
    ("CMAEG_CHANNEL_CACHE_TTL_MS", "60000"),
    ("CMAEG_CHANNEL_NEGATIVE_TTL_MS", "10000"),
    ("CMAEG_CMA_BASE_URL", "http://cma-api.chrono-bot.svc"),
    ("CMAEG_CONSOLE_ACTIVITY_MAX_BYTES", "33554432"),
    ("CMAEG_CONSOLE_ACTIVITY_PER_CHANNEL", "100"),
    ("CMAEG_CONSOLE_CHANNEL_IDS_MAX", "20000"),
    (
        "CMAEG_CONSOLE_CLIENT_ID",
        "33333333-3333-4333-8333-333333333333",
    ),
    ("CMAEG_CONSOLE_ENABLED", "true"),
    ("CMAEG_CONSOLE_INDEX_MAX_RECORDS", "100000"),
    ("CMAEG_CONSOLE_INDEX_RESYNC_MS", "600000"),
    ("CMAEG_CONSOLE_LOGIN_RATE_PER_MINUTE", "60"),
    ("CMAEG_CONSOLE_NYXID_PUBLIC_URL", "https://api.nyxid.ai"),
    ("CMAEG_CONSOLE_PRODUCERS_PER_CHANNEL", "32"),
    ("CMAEG_CONSOLE_PUBLIC_URL", "https://cma.nyxid.ai"),
    ("CMAEG_CONSOLE_SESSIONS_MAX", "1000"),
    ("CMAEG_CONSOLE_SESSIONS_PER_SUBJECT", "5"),
    ("CMAEG_CONSOLE_SESSION_IDLE_MS", "1800000"),
    ("CMAEG_CONSOLE_SESSION_TTL_MS", "28800000"),
    ("CMAEG_CONSOLE_SHARED_SESSIONS", "true"),
    ("CMAEG_CONSOLE_SUMMARY_CACHE_MAX", "2000"),
    ("CMAEG_CONSUMER_INTERACTIONS_ENABLED", "true"),
    ("CMAEG_DRAIN_SECONDS", "660"),
    (
        "CMAEG_FAILURE_TEXT",
        "Sorry, I couldn’t finish that request. Please try again.",
    ),
    ("CMAEG_HEARTBEAT_MS", "60000"),
    ("CMAEG_INGRESS_INFLIGHT", "256"),
    ("CMAEG_INGRESS_MAX_BODY", "1048576"),
    ("CMAEG_JWKS_TTL_MS", "600000"),
    ("CMAEG_LARK_CARDS_ENABLED", "true"),
    ("CMAEG_LISTEN", "0.0.0.0:8090"),
    ("CMAEG_MEDIA_CACHE_BYTES", "33554432"),
    ("CMAEG_MEDIA_CONTINUATIONS_ENABLED", "true"),
    ("CMAEG_MEDIA_DOWNLOAD_BYTES_PER_HOUR", "268435456"),
    ("CMAEG_MEDIA_ENABLED", "true"),
    // The job sandbox, and so a transformer directory, exists only there.
    (
        "CMAEG_MEDIA_FFMPEG_DIR",
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            "/opt/ffmpeg/bin"
        } else {
            ""
        },
    ),
    ("CMAEG_MEDIA_INFLIGHT_BYTES", "67108864"),
    ("CMAEG_MEDIA_LINK_TTL_SECONDS", "120"),
    ("CMAEG_MEDIA_MAX_BYTES", "20971520"),
    ("CMAEG_MEDIA_MAX_JOBS", "6"),
    ("CMAEG_MEDIA_PREPARE_BUDGET_MS", "15000"),
    ("CMAEG_MEDIA_RATE_PER_MINUTE", "60"),
    ("CMAEG_MEDIA_RESPONSE_BYTES", "25165824"),
    ("CMAEG_MEDIA_TOOL_PROVIDERS", "cma_codex"),
    ("CMAEG_MEDIA_VIDEO_FRAMES", "6"),
    ("CMAEG_METRICS_LISTEN", "0.0.0.0:9091"),
    ("CMAEG_MONGODB_AUTHORITY_MODE", "shared-schema"),
    ("CMAEG_MONGODB_DATABASE", "chrono-bot"),
    ("CMAEG_NATIVE_DISCORD_CHANNEL_RECORDS", ""),
    ("CMAEG_NON_HUMAN_PROVIDERS", "cma_codex"),
    ("CMAEG_NYXID_BASE_URL", "https://nyx-api.chrono-ai.fun"),
    ("CMAEG_NYXID_COMPLETE_REPLIES", "false"),
    ("CMAEG_NYXID_NATIVE_MEDIA_ENABLED", "true"),
    ("CMAEG_NYXID_REPLY_TIMEOUT_MS", "5000"),
    ("CMAEG_OWNER_KEYS_ENABLED", "true"),
    (
        "CMAEG_OWNER_KEY_CUTOVER_ID",
        "22222222-2222-4222-8222-222222222222",
    ),
    (
        "CMAEG_OWNER_KEY_SERVICE_ID",
        "11111111-1111-4111-8111-111111111111",
    ),
    ("CMAEG_PROVIDER_CONNECT_TIMEOUT_MS", "2000"),
    ("CMAEG_PROVIDER_MAX_INFLIGHT", "256"),
    ("CMAEG_PROVIDER_REQUEST_TIMEOUT_MS", "30000"),
    (
        "CMAEG_PUBLIC_API_ALIASES",
        "https://bot.chrono-ai.fun/event-gateway",
    ),
    ("CMAEG_PUBLIC_API_URL", "https://cma.nyxid.ai/event-gateway"),
    ("CMAEG_PUBLIC_URL", "https://bot-gateway.chrono-ai.fun"),
    ("CMAEG_RELAY_TEXT_EDITS_ENABLED", "true"),
    ("CMAEG_REPLICAS_ENABLED", "true"),
    ("CMAEG_REPLICA_ADDRESS", "http://10.0.0.10:8090"),
    ("CMAEG_RESTORE_RATE_PER_MINUTE", "10"),
    ("CMAEG_RESTORE_TIMEOUT_MS", "800"),
    (
        "CMAEG_ROUTED_CHANNEL_RECORDS",
        "44444444-4444-4444-8444-444444444441,44444444-4444-4444-8444-444444444442,\
         44444444-4444-4444-8444-444444444443",
    ),
    ("CMAEG_STT_CHANNEL_SECONDS_PER_HOUR", "3600"),
    ("CMAEG_STT_EAGER_MAX_SECONDS", "300"),
    ("CMAEG_STT_FORMATS", "mp3,mp4,mpeg,mpga,m4a,wav,webm"),
    ("CMAEG_STT_MAX_BYTES", "20971520"),
    ("CMAEG_STT_MAX_SECONDS", "600"),
    ("CMAEG_STT_MODEL", "gpt-transcribe"),
    ("CMAEG_STT_RATE_PER_MINUTE", "20"),
    ("CMAEG_STT_SECONDS_PER_HOUR", "36000"),
    ("CMAEG_STT_TIMEOUT_MS", "30000"),
    (
        "CMAEG_STT_URL",
        "https://nyx-api.chrono-ai.fun/api/v1/proxy/s/llm-openai/audio/transcriptions",
    ),
    ("CMAEG_TELEGRAM_EFFECTS_ENABLED", "false"),
    ("CMAEG_TELEGRAM_MESSAGE_EFFECTS_ENABLED", "true"),
    ("CMAEG_TOOLS_NYXID_SLUG", "cmaeg"),
    ("CMAEG_TURN_TIMEOUT_MS", "600000"),
    ("CMAEG_WHATSAPP_INTERACTIONS_ENABLED", "true"),
    ("CMAEG_WHATSAPP_TEMPLATES_ENABLED", "true"),
    ("CMAEG_WORKER_CREDENTIAL_TRACKING_ENABLED", "true"),
];

/// Production's provider registry: CMA with recovery and preparation paths.
fn production_providers() -> String {
    serde_json::json!([
        {
            "slug": "cma_codex",
            "base_url": "http://cma-api.chrono-bot.svc",
            "kind": "responses_http",
            "paths": {
                "agent_card": "/.well-known/agent-card.json",
                "binding": "/api/v1/cmaeg/bindings/{binding_id}",
                "conversation": "/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}",
                "responses": "/api/v2/responses",
                "recovery": {
                    "response": "/api/v2/responses/{response_id}",
                    "events": "/api/v2/responses/{response_id}/events"
                },
                "preparation": {
                    "inspection": "/api/v1/cmaeg/bindings/{binding_id}/preparation-inspections",
                    "reservation": "/api/v1/cmaeg/bindings/{binding_id}/preparations"
                }
            },
            "max_inflight": 256
        },
        {
            "slug": "nyxbot",
            "base_url": "https://nyx-api.chrono-ai.fun",
            "kind": "responses_http",
            "paths": {
                "agent_card": "/api/v1/nyxbot/agent-card",
                "binding": "/api/v1/nyxbot/bindings/{binding_id}",
                "conversation": "/api/v1/nyxbot/bindings/{binding_id}/conversations/{conversation_id}",
                "responses": "/api/v1/nyxbot/responses",
                "recovery": {
                    "response": "/api/v1/nyxbot/responses/{response_id}",
                    "events": "/api/v1/nyxbot/responses/{response_id}/events"
                }
            },
            "max_inflight": 256
        }
    ])
    .to_string()
}

/// The secret names staging's Event Gateway receives, exactly.
const STAGING_SECRETS: &[&str] = &[
    "CMAEG_CMA_CLIENT_ID",
    "CMAEG_CMA_CLIENT_SECRET",
    "CMAEG_CMA_SERVICE_TOKEN",
    "CMAEG_MASTER_KEYS",
    "CMAEG_MONGODB_URI",
    "CMAEG_NYXID_CLIENT_ID",
    "CMAEG_NYXID_CLIENT_SECRET",
];

/// The secret names production's Event Gateway receives, exactly.
const PRODUCTION_SECRETS: &[&str] = &[
    "CMAEG_CMA_CLIENT_ID",
    "CMAEG_CMA_CLIENT_SECRET",
    "CMAEG_CMA_SERVICE_TOKEN",
    "CMAEG_CONSOLE_CLIENT_SECRET",
    "CMAEG_MASTER_KEYS",
    "CMAEG_MONGODB_URI",
    "CMAEG_NYXID_CLIENT_ID",
    "CMAEG_NYXID_CLIENT_SECRET",
    "CMAEG_STT_API_KEY",
];

/// A synthetic placeholder for each named secret.
fn secrets(names: &[&str]) -> Vec<(String, String)> {
    use base64::Engine as _;
    names
        .iter()
        .map(|name| {
            let value = match *name {
                "CMAEG_MASTER_KEYS" => format!(
                    "1:{}",
                    base64::engine::general_purpose::STANDARD_NO_PAD.encode([7_u8; 32])
                ),
                "CMAEG_MONGODB_URI" => "mongodb://127.0.0.1:27017".to_owned(),
                _ => "placeholder".to_owned(),
            };
            ((*name).to_owned(), value)
        })
        .collect()
}

const PROBE: &str = "CMAEG_TEST_GRADUATION_PROBE";
const PROBED: &str = "graduation probe loaded";

/// Load every startup configuration in a child process holding exactly
/// `base`, placeholders for `secret_names` and `overrides`, and check each
/// graduated gate there.
fn probe(
    label: &str,
    base: &[(&str, &str)],
    secret_names: &[&str],
    overrides: &[(&str, &str)],
    mode: &str,
) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "config::graduated::tests::environment_probe",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear();
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let output = command
        .envs(base.iter().copied())
        .envs(secrets(secret_names))
        .envs(overrides.iter().copied())
        .env(PROBE, mode)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains(PROBED),
        "{label}: {stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn staging_loads_with_graduated_gates_unset() {
    probe(
        "staging, graduated gates unset",
        STAGING,
        STAGING_SECRETS,
        &[],
        "on",
    );
}

#[test]
fn staging_loads_with_every_graduated_gate_explicitly_false() {
    let off: Vec<_> = names().map(|name| (name, "false")).collect();
    probe(
        "staging, every graduated gate false",
        STAGING,
        STAGING_SECRETS,
        &off,
        "off",
    );
}

#[test]
fn staging_loads_with_each_graduated_gate_alone_explicitly_false() {
    probe(
        "staging, each graduated gate false",
        STAGING,
        STAGING_SECRETS,
        &[],
        "each",
    );
}

#[test]
fn production_loads_without_its_graduated_entries() {
    let providers = production_providers();
    probe(
        "production, graduated gates unset",
        PRODUCTION,
        PRODUCTION_SECRETS,
        &[("CMAEG_PROVIDERS", &providers)],
        "on",
    );
}

/// Every graduated gate as the started service reads it.
fn observed() -> Result<BTreeMap<&'static str, bool>, String> {
    use crate::config as c;
    let error = |error: ConfigError| error.to_string();
    // `crate::run` reads these before the configuration, and fails on any.
    c::CommonmarkRelay::from_env().map_err(error)?;
    c::nyxid_complete_replies_from_env().map_err(error)?;
    c::nyxid_native_media_from_env().map_err(error)?;
    c::relay_text_edits_from_env().map_err(error)?;
    c::lark_cards_from_env().map_err(error)?;
    c::consumer_interactions_from_env().map_err(error)?;
    c::whatsapp_interactions_from_env().map_err(error)?;
    c::whatsapp_templates_from_env().map_err(error)?;
    c::telegram_message_effects_from_env().map_err(error)?;
    let mut observed = BTreeMap::from([
        (
            "CMAEG_NATIVE_DISCORD_EDITS_ENABLED",
            c::native_discord_edits_from_env().map_err(error)?,
        ),
        (
            "CMAEG_NATIVE_DISCORD_MESSAGE_EFFECTS_ENABLED",
            c::native_discord_effects_from_env().map_err(error)?,
        ),
        (
            "CMAEG_NATIVE_DISCORD_MEDIA_EFFECTS_ENABLED",
            c::native_discord_media_effects_from_env().map_err(error)?,
        ),
        (
            "CMAEG_NATIVE_DISCORD_THREAD_METADATA_ENABLED",
            c::native_discord_thread_metadata_from_env().map_err(error)?,
        ),
        (
            "CMAEG_LARK_FORMS_ENABLED",
            c::lark_forms_from_env().map_err(error)?,
        ),
        (
            "CMAEG_DISCORD_INTERACTIONS_ENABLED",
            c::discord_interactions_from_env().map_err(error)?,
        ),
        (
            "CMAEG_DISCORD_FORMS_ENABLED",
            c::discord_forms_from_env().map_err(error)?,
        ),
        (
            "CMAEG_DISCORD_CARD_EFFECTS_ENABLED",
            c::discord_card_effects_from_env().map_err(error)?,
        ),
        (
            "CMAEG_DISCORD_CARD_EDITS_ENABLED",
            c::discord_card_edits_from_env().map_err(error)?,
        ),
        (
            "CMAEG_DISCORD_MULTISELECT_ENABLED",
            c::discord_multiselect_from_env().map_err(error)?,
        ),
        (
            "CMAEG_DISCORD_MESSAGE_EVENTS_ENABLED",
            c::discord_message_events_from_env().map_err(error)?,
        ),
        (
            "CMAEG_TELEGRAM_MEDIA_EFFECTS_ENABLED",
            c::telegram_media_effects_from_env().map_err(error)?,
        ),
    ]);
    let config = c::Config::from_env().map_err(error)?;
    // `serve` and storage composition read these before any network call.
    let replicas = crate::replicas::config::Config::from_env()?;
    config
        .console
        .validate_replica_mode(replicas.enabled)
        .map_err(error)?;
    crate::channels::mutation::enabled_from_env().map_err(error)?;
    c::owner_keys::Settings::from_env().map_err(|error| error.to_string())?;
    let routing = &config.routing;
    let discord = &config.native_discord;
    observed.extend([
        (
            "CMAEG_CUSTOM_DURABLE_EXECUTION_ENABLED",
            routing.custom_durable_execution_enabled,
        ),
        (
            "CMAEG_NATIVE_QUOTED_CONTEXT_ENABLED",
            routing.quoted_context_enabled,
        ),
        ("CMAEG_ROUTED_ADMISSION_ENABLED", routing.admission_enabled),
        (CMAEG_ROUTED_RECOVERY_ENABLED, routing.recovery_enabled),
        (CMAEG_NATIVE_DISCORD_ENABLED, discord.enabled),
        (
            "CMAEG_NATIVE_DISCORD_DISCOVERY_ENABLED",
            discord.discovery_enabled,
        ),
        (
            "CMAEG_NATIVE_DISCORD_RICH_CONTENT_ENABLED",
            discord.rich_content_enabled,
        ),
        (
            "CMAEG_NATIVE_DISCORD_FORWARDS_ENABLED",
            discord.forwards_enabled,
        ),
        ("CMAEG_GITHUB_ENABLED", config.github_enabled),
        (
            "CMAEG_NATIVE_BOT_MENTIONS_WRITES_ENABLED",
            config.native_bot_mentions_writes_enabled,
        ),
        (
            "CMAEG_THREAD_CONTRACT_WRITES_ENABLED",
            config.thread_contract_writes_enabled,
        ),
        (
            "CMAEG_MULTI_CONSUMER_WRITES_ENABLED",
            config.multi_consumer_writes_enabled,
        ),
        (
            "CMAEG_DYNAMIC_SOURCES_WRITES_ENABLED",
            config.dynamic_sources_writes_enabled,
        ),
        (
            "CMAEG_DISCORD_REACTIONS_ENABLED",
            config.native_discord_reactions_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_CONVERSATION_HISTORY_ENABLED",
            config.native_lark_conversation_history_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_MIXED_HISTORY_ENABLED",
            config.native_lark_mixed_history_enabled,
        ),
        (
            "CMAEG_NATIVE_DISCORD_CONVERSATION_HISTORY_ENABLED",
            config.native_discord_conversation_history_enabled,
        ),
        (
            "CMAEG_NATIVE_TELEGRAM_TOPIC_LIFECYCLE_ENABLED",
            config.native_telegram_topic_lifecycle_enabled,
        ),
        (
            "CMAEG_NATIVE_TELEGRAM_TOPIC_CREATION_ENABLED",
            config.native_telegram_topic_creation_enabled,
        ),
        (
            "CMAEG_NATIVE_TELEGRAM_GENERAL_TOPIC_LIFECYCLE_ENABLED",
            config.native_telegram_general_topic_lifecycle_enabled,
        ),
        (
            "CMAEG_NATIVE_DISCORD_THREAD_LIFECYCLE_ENABLED",
            config.native_discord_thread_lifecycle_enabled,
        ),
        (
            "CMAEG_NATIVE_DISCORD_THREAD_HISTORY_ENABLED",
            config.native_discord_thread_history_enabled,
        ),
        (
            "CMAEG_NATIVE_TELEGRAM_MESSAGE_EVENTS_WRITES_ENABLED",
            config.native_telegram_message_events_writes_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_WRITES_ENABLED",
            config.native_lark_writes_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_INTAKE_ENABLED",
            config.native_lark_intake_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_MEDIA_ENABLED",
            config.native_lark_media_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_STICKERS_ENABLED",
            config.native_lark_stickers_enabled,
        ),
        (
            "CMAEG_NATIVE_WHATSAPP_GROUPS_WRITES_ENABLED",
            config.native_whatsapp_groups_writes_enabled,
        ),
        (
            "CMAEG_NATIVE_WHATSAPP_GROUPS_ENABLED",
            config.native_whatsapp_groups_enabled,
        ),
        (
            "CMAEG_NATIVE_TELEGRAM_MEDIA_CONTROL_EDITS_ENABLED",
            config.native_telegram_media_control_edits_enabled,
        ),
        (
            "CMAEG_NATIVE_TELEGRAM_MEDIA_CONTROLS_ENABLED",
            config.native_telegram_media_controls_enabled,
        ),
        (
            "CMAEG_NATIVE_SPLIT_TEXT_REFS_ENABLED",
            config.native_split_text_refs_enabled,
        ),
        (
            "CMAEG_NATIVE_TELEGRAM_MEDIA_EDITS_ENABLED",
            config.native_telegram_media_edits_enabled,
        ),
        (
            "CMAEG_NATIVE_DISCORD_MEDIA_CONTROLS_ENABLED",
            config.native_discord_media_controls_enabled,
        ),
        (
            "CMAEG_NATIVE_DISCORD_MEDIA_CARD_EDITS_ENABLED",
            config.native_discord_media_card_edits_enabled,
        ),
        (
            "CMAEG_NATIVE_DISCORD_MEDIA_CARD_SETS_ENABLED",
            config.native_discord_media_card_sets_enabled,
        ),
        (
            "CMAEG_NATIVE_DISCORD_MEDIA_CARD_REPLACEMENTS_ENABLED",
            config.native_discord_media_card_replacements_enabled,
        ),
        (
            "CMAEG_NATIVE_DISCORD_MEDIA_EDITS_ENABLED",
            config.native_discord_media_edits_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_CONTROLS_ENABLED",
            config.native_lark_controls_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_FORMS_ENABLED",
            config.native_lark_forms_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_MESSAGE_EFFECTS_ENABLED",
            config.native_lark_message_effects_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_RECEIVED_REACTIONS_ENABLED",
            config.native_lark_received_reactions_enabled,
        ),
        (
            "CMAEG_NATIVE_WHATSAPP_MESSAGE_EVENTS_ENABLED",
            config.native_whatsapp_message_events_enabled,
        ),
        (
            "CMAEG_NATIVE_WHATSAPP_FORMS_ENABLED",
            config.native_whatsapp_forms_enabled,
        ),
        (
            "CMAEG_NATIVE_WHATSAPP_FLOW_WRITES_ENABLED",
            config.native_whatsapp_flow_writes_enabled,
        ),
        ("CMAEG_NATIVE_TYPING_ENABLED", config.native_typing_enabled),
        (
            "CMAEG_NATIVE_WHATSAPP_WRITES_ENABLED",
            config.native_whatsapp_writes_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_THREAD_START_ENABLED",
            config.native_lark_thread_start_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_MEDIA_CONTROLS_ENABLED",
            config.native_lark_media_controls_enabled,
        ),
        (
            "CMAEG_NATIVE_MEDIA_PRESENTATIONS_ENABLED",
            config.native_media_presentations_enabled,
        ),
        (
            "CMAEG_NATIVE_TELEGRAM_STRUCTURED_REPLIES_ENABLED",
            config.native_telegram_structured_replies_enabled,
        ),
        (
            "CMAEG_NATIVE_WHATSAPP_STRUCTURED_REPLIES_ENABLED",
            config.native_whatsapp_structured_replies_enabled,
        ),
        (
            "CMAEG_NATIVE_TELEGRAM_ALBUMS_ENABLED",
            config.native_telegram_albums_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_MULTI_IMAGE_CONTROLS_ENABLED",
            config.native_lark_multi_image_controls_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_MEDIA_CARD_REPLACEMENTS_ENABLED",
            config.native_lark_media_card_replacements_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_OUTBOUND_MEDIA_ENABLED",
            config.native_lark_outbound_media_enabled,
        ),
        (
            "CMAEG_NATIVE_LARK_STICKER_REPLIES_ENABLED",
            config.native_lark_sticker_replies_enabled,
        ),
        (
            "CMAEG_NATIVE_WHATSAPP_OUTBOUND_MEDIA_ENABLED",
            config.native_whatsapp_outbound_media_enabled,
        ),
        (
            "CMAEG_NATIVE_WHATSAPP_INTERACTIONS_ENABLED",
            config.native_whatsapp_interactions_enabled,
        ),
        (
            "CMAEG_NATIVE_WHATSAPP_MEDIA_CONTROLS_ENABLED",
            config.native_whatsapp_media_controls_enabled,
        ),
        (
            "CMAEG_NATIVE_WHATSAPP_REACTIONS_ENABLED",
            config.native_whatsapp_reactions_enabled,
        ),
        (
            "CMAEG_NATIVE_TELEGRAM_RECEIVED_REACTIONS_ENABLED",
            config.native_telegram_received_reactions_enabled,
        ),
        (
            "CMAEG_NATIVE_DISCORD_RECEIVED_REACTIONS_ENABLED",
            config.native_discord_received_reactions_enabled,
        ),
        (
            "CMAEG_NATIVE_WHATSAPP_TEMPLATES_ENABLED",
            config.native_whatsapp_templates_enabled,
        ),
        (
            "CMAEG_NATIVE_WHATSAPP_MEDIA_ENABLED",
            config.native_whatsapp_media_enabled,
        ),
        (
            "CMAEG_NATIVE_DISCORD_WRITES_ENABLED",
            config.native_discord_writes_enabled,
        ),
        (
            "CMAEG_RELAY_RAW_VERIFICATION_WRITES_ENABLED",
            config.relay_raw_verification_writes_enabled,
        ),
        (
            "CMAEG_LARK_MESSAGE_EFFECTS_WRITES_ENABLED",
            config.lark_message_effects_writes_enabled,
        ),
        (
            "CMAEG_LARK_RESOURCE_WRITES_ENABLED",
            config.lark_resource_writes_enabled,
        ),
        (
            "CMAEG_OUTBOUND_MESSAGES_WRITES_ENABLED",
            config.outbound_messages_writes_enabled,
        ),
    ]);
    Ok(observed)
}

/// Check that the loaded gates match: every gate except those switched off
/// (and their unset dependents) is on.
fn check(off: &[&str]) {
    let observed = observed().unwrap_or_else(|error| panic!("off={off:?}: {error}"));
    let observed_names: Vec<_> = observed.keys().copied().collect();
    let mut graduated: Vec<_> = names().collect();
    graduated.sort_unstable();
    assert_eq!(
        observed_names, graduated,
        "every graduated gate is observed"
    );
    for (name, enabled) in observed {
        let expected = !off.iter().any(|gate| depends_on(name, gate));
        assert_eq!(enabled, expected, "{name} with {off:?} switched off");
    }
}

#[test]
#[ignore = "runs only inside the environment probe child process"]
fn environment_probe() {
    let Ok(mode) = std::env::var(PROBE) else {
        return;
    };
    match mode.as_str() {
        "on" => check(&[]),
        "off" => check(&names().collect::<Vec<_>>()),
        // The child is single-threaded, so it may change its own environment.
        "each" => {
            check(&[]);
            for name in names() {
                std::env::set_var(name, "false");
                check(&[name]);
                std::env::remove_var(name);
            }
        }
        other => panic!("unknown probe mode {other}"),
    }
    println!("{PROBED}");
}

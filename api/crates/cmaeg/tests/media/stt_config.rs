//! The speech-to-text endpoint settings: the URL policy, the platform key,
//! the model, the formats and the attempt timeout.
use super::stt_fake::{config_for, KEY};
use cmaeg::{ConfigError, SttConfig};
use std::time::Duration;

const LOCAL: &str = "http://127.0.0.1:8099/v1/audio/transcriptions";

fn lookup(pairs: &[(&'static str, &str)]) -> Result<SttConfig, ConfigError> {
    let pairs: Vec<(&'static str, String)> = pairs
        .iter()
        .map(|(key, value)| (*key, (*value).to_owned()))
        .collect();
    SttConfig::from_lookup(|key| {
        Ok(pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.clone()))
    })
}

fn detail(error: ConfigError) -> String {
    match error {
        ConfigError::InvalidMedia { detail } => detail,
        other => panic!("expected an invalid media configuration, got {other}"),
    }
}

#[test]
fn speech_to_text_is_off_with_documented_defaults() {
    let config = lookup(&[]).unwrap();
    assert!(config.endpoint.is_none());
    assert_eq!(config.model, "gpt-transcribe");
    assert_eq!(
        config.formats.iter().copied().collect::<Vec<_>>(),
        ["m4a", "mp3", "mp4", "mpeg", "mpga", "wav", "webm"]
    );
    assert_eq!(config.timeout, Duration::from_millis(30_000));
    assert_eq!(config, SttConfig::default());
    assert!(format!("{config:?}").contains("url_configured: false"));
}

#[test]
fn an_empty_url_ignores_the_key_whatever_it_holds() {
    for key in ["has whitespace", "", KEY] {
        let config = lookup(&[("CMAEG_STT_URL", ""), ("CMAEG_STT_API_KEY", key)]).unwrap();
        assert!(config.endpoint.is_none());
    }
    assert!(lookup(&[("CMAEG_STT_API_KEY", "has whitespace")])
        .unwrap()
        .endpoint
        .is_none());
}

#[test]
fn https_and_loopback_http_urls_are_accepted() {
    for url in [
        LOCAL,
        "http://localhost:8099/v1/audio/transcriptions",
        "http://[::1]:8099/v1/audio/transcriptions",
        "https://api.example/v1/audio/transcriptions",
        "https://stt.example",
    ] {
        let config = config_for(url, &[]).unwrap();
        let endpoint = config.endpoint.expect("an endpoint");
        assert_eq!(endpoint.url.as_str().trim_end_matches('/'), url);
        assert_eq!(endpoint.api_key.as_str(), KEY);
    }
}

#[test]
fn unsafe_urls_are_refused_without_echoing_them() {
    for url in [
        "http://stt.example/v1/audio/transcriptions",
        "http://10.0.0.1/v1/audio/transcriptions",
        "https://user:pass@stt.example/v1/audio/transcriptions",
        "https://user@stt.example/v1/audio/transcriptions",
        "https://stt.example/v1/audio/transcriptions?key=1",
        "https://stt.example/v1/audio/transcriptions#part",
        "ftp://stt.example/upload",
        "file:///tmp/stt",
        "/v1/audio/transcriptions",
        "not a url",
    ] {
        let shown = detail(config_for(url, &[]).unwrap_err());
        assert!(
            shown.starts_with("CMAEG_STT_URL must be an absolute https URL"),
            "{url}: {shown}"
        );
        assert!(!shown.contains("stt.example"), "{shown}");
        assert!(!shown.contains(KEY), "{shown}");
    }
}

#[test]
fn a_url_requires_a_printable_bounded_key() {
    let missing = detail(lookup(&[("CMAEG_STT_URL", LOCAL)]).unwrap_err());
    assert_eq!(
        missing,
        "CMAEG_STT_API_KEY must be set when CMAEG_STT_URL is set"
    );
    assert!(!missing.contains("127.0.0.1"));
    let invalid =
        "CMAEG_STT_API_KEY must be 1 to 4096 printable ASCII characters without whitespace";
    let long = "k".repeat(4097);
    for key in [
        "",
        "two words",
        "tab\tkey",
        "line\nkey",
        "caf\u{e9}",
        long.as_str(),
    ] {
        let error = lookup(&[("CMAEG_STT_URL", LOCAL), ("CMAEG_STT_API_KEY", key)]).unwrap_err();
        assert_eq!(detail(error), invalid, "{key:?}");
    }
    let longest = "k".repeat(4096);
    let config = lookup(&[("CMAEG_STT_URL", LOCAL), ("CMAEG_STT_API_KEY", &longest)]).unwrap();
    assert_eq!(config.endpoint.unwrap().api_key.as_str(), longest);
}

#[test]
fn formats_are_a_list_of_distinct_known_names() {
    let all = "mp3,mp4,mpeg,mpga,m4a,wav,webm,ogg,flac";
    assert_eq!(
        config_for(LOCAL, &[("CMAEG_STT_FORMATS", all)])
            .unwrap()
            .formats
            .len(),
        9
    );
    let ogg = config_for(LOCAL, &[("CMAEG_STT_FORMATS", "ogg")]).unwrap();
    assert_eq!(ogg.formats.iter().copied().collect::<Vec<_>>(), ["ogg"]);
    let unknown = "CMAEG_STT_FORMATS must list formats from mp3, mp4, mpeg, mpga, m4a, wav, \
                   webm, ogg, flac separated by commas, without empty entries";
    for value in [
        "", "aac", "mp3,", ",mp3", "mp3,,wav", "MP3", " mp3", "mp3 ,wav", "opus",
    ] {
        let error = config_for(LOCAL, &[("CMAEG_STT_FORMATS", value)]).unwrap_err();
        assert_eq!(detail(error), unknown, "{value:?}");
    }
    let error = config_for(LOCAL, &[("CMAEG_STT_FORMATS", "mp3,wav,mp3")]).unwrap_err();
    assert_eq!(detail(error), "CMAEG_STT_FORMATS lists mp3 more than once");
    // Formats are validated while speech-to-text is off, too.
    assert!(lookup(&[("CMAEG_STT_FORMATS", "aac")]).is_err());
}

#[test]
fn the_attempt_timeout_is_bounded() {
    for (value, millis) in [("1000", 1_000), ("120000", 120_000)] {
        let config = config_for(LOCAL, &[("CMAEG_STT_TIMEOUT_MS", value)]).unwrap();
        assert_eq!(config.timeout, Duration::from_millis(millis));
    }
    for value in ["999", "120001", "", "30s", "-1"] {
        let error = config_for(LOCAL, &[("CMAEG_STT_TIMEOUT_MS", value)]).unwrap_err();
        assert_eq!(
            detail(error),
            "CMAEG_STT_TIMEOUT_MS must be an unsigned decimal from 1000 to 120000",
            "{value:?}"
        );
    }
}

#[test]
fn the_model_is_bounded_printable_ascii() {
    let longest = "m".repeat(128);
    for model in ["whisper-1", "gpt-4o-mini-transcribe", longest.as_str()] {
        let config = config_for(LOCAL, &[("CMAEG_STT_MODEL", model)]).unwrap();
        assert_eq!(config.model, model);
    }
    let too_long = "m".repeat(129);
    for model in [
        "",
        "gpt transcribe",
        "tab\tmodel",
        "mod\u{e8}le",
        too_long.as_str(),
    ] {
        let error = config_for(LOCAL, &[("CMAEG_STT_MODEL", model)]).unwrap_err();
        assert_eq!(
            detail(error),
            "CMAEG_STT_MODEL must be 1 to 128 printable ASCII characters without whitespace",
            "{model:?}"
        );
    }
}

#[test]
fn the_eager_audio_threshold_is_bounded_and_zero_disables_it() {
    assert_eq!(lookup(&[]).unwrap().eager_max_seconds, 300);
    for (value, seconds) in [("0", 0), ("600", 600), ("60", 60)] {
        let config = lookup(&[("CMAEG_STT_EAGER_MAX_SECONDS", value)]).unwrap();
        assert_eq!(config.eager_max_seconds, seconds);
    }
    for value in ["601", "-1", "", "5m"] {
        assert_eq!(
            detail(lookup(&[("CMAEG_STT_EAGER_MAX_SECONDS", value)]).unwrap_err()),
            "CMAEG_STT_EAGER_MAX_SECONDS must be an unsigned decimal from 0 to 600",
            "{value:?}"
        );
    }
    let config = lookup(&[("CMAEG_STT_EAGER_MAX_SECONDS", "120")]).unwrap();
    assert!(format!("{config:?}").contains("eager_max_seconds: 120"));
}

use serde_json::{json, Value};

/// Preserve the exact health contract and independently check the binary's stamp.
pub fn assert_readiness(mut response: Value, health: Value) {
    let build = response.as_object_mut().unwrap().remove("build");
    assert_eq!(response, health);
    match (option_env!("CHRONO_BUILD_SHA"), option_env!("CHRONO_BUILD")) {
        (Some(sha), Some(version))
            if sha.len() == 40
                && sha
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
                && !sha.trim_matches('0').is_empty()
                && !version.is_empty() =>
        {
            assert_eq!(build, Some(json!({"source_sha": sha, "version": version})));
        }
        _ => assert!(build.is_none()),
    }
}

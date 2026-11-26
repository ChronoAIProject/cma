//! The CMA workflow paths a sign-in may return to.

// Public budget authority: layer1-public/tests/fixtures/browser-auth-limits.json.
// Independent SDKs replay the same boundary; the web imports the TS SDK constant.
pub(super) const MAX_LOGIN_URL_BYTES: usize = 4096;

pub(in crate::auth) fn app_return_target(value: &str) -> Result<String, ()> {
    return_target(value, false)
}

pub(super) fn interactive_return_target(value: &str) -> Result<String, ()> {
    return_target(value, true)
}

fn return_target(value: &str, allow_agent_share: bool) -> Result<String, ()> {
    let (path, query) = value
        .split_once('?')
        .map_or((value, None), |(path, query)| (path, Some(query)));
    if value.len() > MAX_LOGIN_URL_BYTES
        || !value.is_ascii()
        || value.contains(['#', '\\'])
        || value.bytes().any(|b| b.is_ascii_control() || b == b' ')
    {
        return Err(());
    }
    if let Some(query) = query {
        let mut keys = std::collections::HashSet::new();
        for (key, _) in url::form_urlencoded::parse(query.as_bytes()) {
            if !keys.insert(key.clone())
                || !((path == "/bots/new" && key == "setup")
                    || (path.starts_with("/sandboxes/") && key == "tab"))
            {
                return Err(());
            }
        }
    }
    if path == "/" {
        return Ok(value.to_owned());
    }
    if path.len() > 256 || path.contains('%') {
        return Err(());
    }
    let segments = path
        .strip_prefix('/')
        .ok_or(())?
        .split('/')
        .collect::<Vec<_>>();
    if segments.iter().any(|segment| !opaque_path_segment(segment)) {
        return Err(());
    }
    match segments.as_slice() {
        ["agent-manager" | "event-gateway" | "image-manager" | "docs" | "usage"] => {}
        ["agent-manager", "share", _] if allow_agent_share && query.is_none() => {}
        ["docs", "event-gateway"] => {}
        ["t", public_id] if opaque_path_segment(public_id) => {}
        ["t", public_id, placement_id]
            if opaque_path_segment(public_id) && opaque_path_segment(placement_id) => {}
        ["triggers", "manage"] => {}
        ["triggers", "manage", trigger_id] if opaque_path_segment(trigger_id) => {}
        ["triggers", "manage", trigger_id, "embed"] if opaque_path_segment(trigger_id) => {}
        ["agents", agent_id] if opaque_path_segment(agent_id) => {}
        ["sandboxes", sandbox_id] if opaque_path_segment(sandbox_id) => {}
        ["bots"] => {}
        ["bots", bot_id] if opaque_path_segment(bot_id) => {}
        ["auth", "bot-father", "link", intent, "complete"] if opaque_path_segment(intent) => {}
        ["profiles"] | ["marketplace"] => {}
        ["profiles" | "marketplace", kind, profile_id]
            if matches!(*kind, "agent" | "workspace" | "sandbox")
                && opaque_path_segment(profile_id) => {}
        _ => return Err(()),
    }
    Ok(value.to_owned())
}

pub(super) fn opaque_path_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_browser_return_target_contract() {
        #[derive(serde::Deserialize)]
        struct Case {
            target: String,
            explicit: Option<String>,
            backend: bool,
        }
        let cases: Vec<Case> = serde_json::from_str(include_str!(
            "../../../../layer1-public/tests/fixtures/browser-auth-return-targets.json"
        ))
        .unwrap();
        for case in cases {
            assert_eq!(
                interactive_return_target(&case.target).is_ok(),
                case.backend,
                "raw target: {}",
                case.target
            );
            if let Some(emitted) = case.explicit {
                assert_eq!(
                    interactive_return_target(&emitted),
                    Ok(emitted.clone()),
                    "browser destination from {}",
                    case.target
                );
            }
        }
    }

    #[test]
    fn agent_share_requires_interactive_sign_in() {
        let share = "/agent-manager/share/shr_fixture";
        assert_eq!(interactive_return_target(share).as_deref(), Ok(share));
        assert!(app_return_target(share).is_err());
    }

    #[test]
    fn preserves_only_bounded_ui_query_state_without_changing_path_authority() {
        for target in [
            "/sandboxes/sbx_1?tab=access",
            "/bots/new?setup=%7B%22name%22%3A%22draft%22%7D",
        ] {
            assert_eq!(app_return_target(target).unwrap(), target);
        }
        for target in [
            "/sandboxes/sbx_1?tab=access&tab=activity",
            "/bots/new?setup=a&next=%2F%2Fevil.test",
            "/bots/new?token=authority",
            "/bots/new#token=authority",
            "/bots/new\r\nLocation: https://evil.test",
            "/sandboxes/../?tab=access",
            "//evil.test?tab=access",
            "/%2Fexample?tab=access",
        ] {
            assert!(app_return_target(target).is_err(), "{target}");
        }
        assert!(app_return_target(&format!("/bots/new?setup={}", "a".repeat(25065))).is_err());
    }
}

//! The CMA workflow paths a sign-in may return to.

pub(super) fn app_return_target(value: &str) -> Result<String, ()> {
    if value == "/" {
        return Ok(value.to_owned());
    }
    if value.len() > 256 || value.contains(['?', '#', '\\', '%']) {
        return Err(());
    }
    let segments = value
        .strip_prefix('/')
        .ok_or(())?
        .split('/')
        .collect::<Vec<_>>();
    if segments.iter().any(|segment| !opaque_path_segment(segment)) {
        return Err(());
    }
    match segments.as_slice() {
        ["agent-manager" | "event-gateway" | "image-manager"] => {}
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

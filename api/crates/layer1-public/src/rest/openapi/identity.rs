pub(super) fn anonymous_path(path: &str) -> bool {
    matches!(
        path,
        "/auth/signup-options"
            | "/api/v1/conformance"
            | "/api/v1/agui/capabilities"
            | "/api/v1/triggers/public/{public_id}"
            | "/api/v1/triggers/public/{public_id}/badge.svg"
            | "/api/v1/triggers/public/{public_id}/open/{placement_id}"
            | "/api/v1/triggers/public/{public_id}/entry-intent"
    )
}

pub(super) fn operation_id(method: &str, path: &str) -> String {
    let mut id = method.to_owned();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        id.push('_');
        if segment.starts_with('{') && segment.ends_with('}') {
            id.push_str("by_");
        }
        let mut previous_was_separator = false;
        for character in segment.trim_matches(['{', '}']).chars() {
            if character.is_ascii_alphanumeric() || character == '_' {
                id.push(character.to_ascii_lowercase());
                previous_was_separator = false;
            } else if !previous_was_separator {
                id.push('_');
                previous_was_separator = true;
            }
        }
        if id.ends_with('_') {
            id.pop();
        }
    }
    id
}

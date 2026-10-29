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

/// Attach public operation identities and credential schemes to every projection.
pub(super) fn secure_operations(document: &mut utoipa::openapi::OpenApi) {
    use utoipa::openapi::{self, security::SecurityRequirement};
    let components = document
        .components
        .get_or_insert_with(openapi::Components::new);
    components.add_security_scheme(
        "bearerAuth",
        openapi::security::SecurityScheme::Http(
            openapi::security::HttpBuilder::new()
                .scheme(openapi::security::HttpAuthScheme::Bearer)
                .build(),
        ),
    );
    components.add_security_scheme(
        "sessionCookie",
        openapi::security::SecurityScheme::ApiKey(openapi::security::ApiKey::Cookie(
            openapi::security::ApiKeyValue::with_description(
                "cma_session",
                "CMA browser session cookie",
            ),
        )),
    );
    for (path, item) in &mut document.paths.paths {
        for (method, operation) in [
            ("get", item.get.as_mut()),
            ("put", item.put.as_mut()),
            ("post", item.post.as_mut()),
            ("delete", item.delete.as_mut()),
            ("patch", item.patch.as_mut()),
        ]
        .into_iter()
        .filter_map(|(method, operation)| operation.map(|operation| (method, operation)))
        {
            operation.operation_id = Some(operation_id(method, path));
            operation.security = Some(if anonymous_path(path) {
                Vec::new()
            } else if path == crate::rest::worker_credentials::WORKER_CLASSIFIER_PATH {
                vec![SecurityRequirement::new(
                    "bearerAuth",
                    std::iter::empty::<String>(),
                )]
            } else {
                vec![
                    SecurityRequirement::new("bearerAuth", std::iter::empty::<String>()),
                    SecurityRequirement::new("sessionCookie", std::iter::empty::<String>()),
                ]
            });
        }
    }
}

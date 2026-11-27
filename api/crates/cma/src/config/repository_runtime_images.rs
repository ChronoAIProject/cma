//! Optional shared environment entry for the independent repository image lists.

use serde::Deserialize;

const BUNDLE: &str = "CHRONO_REPOSITORY_RUNTIME_IMAGES";
const PUBLIC: &str = "CHRONO_PUBLIC_REPOSITORY_VERIFIED_RUNTIME_IMAGES";
const MANAGED: &str = "CHRONO_GITHUB_REPOSITORY_RUNTIME_IMAGES";

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ImageLists {
    public: Option<String>,
    managed: Option<String>,
}

pub(super) fn with_bundle(
    mut lookup: impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<impl FnMut(&str) -> Result<Option<String>, String>, String> {
    let lists: ImageLists = match lookup(BUNDLE)? {
        Some(raw) => {
            let invalid =
                || format!("{BUNDLE} must be a JSON object with public or managed string fields");
            // Serde also accepts positional arrays for structs. Require named policies.
            if !raw.trim_start().starts_with('{') {
                return Err(invalid());
            }
            serde_json::from_str(&raw).map_err(|_| invalid())?
        }
        None => ImageLists::default(),
    };
    // Use the existing validators and keep each standalone value authoritative.
    Ok(move |key: &str| {
        Ok(lookup(key)?.or_else(|| match key {
            PUBLIC => lists.public.clone(),
            MANAGED => lists.managed.clone(),
            _ => None,
        }))
    })
}

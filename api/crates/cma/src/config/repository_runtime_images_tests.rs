use super::configured;
use serde_json::json;

const BUNDLE: &str = "CHRONO_REPOSITORY_RUNTIME_IMAGES";
const PUBLIC: &str = "CHRONO_PUBLIC_REPOSITORY_VERIFIED_RUNTIME_IMAGES";
const MANAGED: &str = "CHRONO_GITHUB_REPOSITORY_RUNTIME_IMAGES";

fn image(digit: &str) -> String {
    format!("registry.example/runtime@sha256:{}", digit.repeat(64))
}

#[test]
fn startup_loads_separate_repository_image_lists_from_one_entry() {
    let public = image("a");
    let managed = image("b");
    let bundle = json!({"public": public, "managed": managed}).to_string();
    let config = configured(&[(BUNDLE, &bundle)]).unwrap();

    assert_eq!(
        config.public_repository_verified_runtime_images,
        [public].into()
    );
    assert_eq!(config.runtime.github_repository_runtime_images, [managed]);
}

#[test]
fn startup_preserves_standalone_overrides_and_omitted_lists() {
    let bundled = image("a");
    let standalone = image("b");
    let bundle = json!({"public": bundled}).to_string();
    let config = configured(&[(BUNDLE, &bundle), (PUBLIC, &standalone)]).unwrap();

    assert_eq!(
        config.public_repository_verified_runtime_images,
        [standalone.clone()].into()
    );
    assert!(config.runtime.github_repository_runtime_images.is_empty());

    let config = configured(&[(BUNDLE, &bundle), (MANAGED, &standalone)]).unwrap();
    assert_eq!(
        config.public_repository_verified_runtime_images,
        [bundled].into()
    );
    assert_eq!(
        config.runtime.github_repository_runtime_images,
        [standalone]
    );
}

#[test]
fn startup_rejects_unknown_fields_and_unqualified_image_references() {
    let error = configured(&[(BUNDLE, r#"{"other":"redacted-marker"}"#)]).unwrap_err();
    assert!(error.contains(BUNDLE));
    assert!(!error.contains("redacted-marker"));

    for (field, legacy_key) in [("public", PUBLIC), ("managed", MANAGED)] {
        let bundle = json!({(field): "registry.example/runtime:latest"}).to_string();
        assert!(configured(&[(BUNDLE, &bundle)])
            .unwrap_err()
            .contains(legacy_key));
    }
}

#[test]
fn startup_requires_named_repository_image_policies() {
    let qualified = image("a");
    for array in [json!([qualified, null]), json!([null, qualified])] {
        let error = configured(&[(BUNDLE, &array.to_string())]).unwrap_err();
        assert!(error.contains(BUNDLE));
        assert!(!error.contains(&qualified));
    }
}

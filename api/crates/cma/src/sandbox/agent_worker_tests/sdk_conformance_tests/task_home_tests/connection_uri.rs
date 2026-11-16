//! Rebind an authenticated fixture connection to its isolated runtime principal.

pub(super) fn runtime_uri(
    bootstrap: &str,
    username: &str,
    password: &str,
    database: &str,
) -> reqwest::Url {
    let mut uri = reqwest::Url::parse(bootstrap).unwrap();
    uri.set_username(username).unwrap();
    uri.set_password(Some(password)).unwrap();
    // Authenticated fixtures already name an auth database. Mongo options are
    // case-insensitive, and appending another authSource rejects the whole URI.
    // Preserve unrelated encoded values: Mongo decodes %20 as a space but
    // keeps a literal '+', unlike form-style query serialization.
    let options = uri
        .query()
        .unwrap_or_default()
        .split('&')
        .filter(|pair| !pair.is_empty())
        .filter(|pair| {
            !pair
                .split_once('=')
                .is_some_and(|(key, _)| key.eq_ignore_ascii_case("authSource"))
        })
        .collect::<Vec<_>>()
        .join("&");
    uri.set_query((!options.is_empty()).then_some(options.as_str()));
    uri.query_pairs_mut().append_pair("authSource", database);
    uri
}

#[tokio::test]
async fn authenticated_bootstrap_rebinds_to_the_runtime_database() {
    let uri = runtime_uri(
        "mongodb://bootstrap:fixture@127.0.0.1:27017/?authSource=admin&replicaSet=fixture&directConnection=true",
        "task-runtime",
        "runtime-fixture",
        "isolated_task",
    );
    let options = mongodb::options::ClientOptions::parse(uri.as_str())
        .await
        .unwrap();
    let credential = options.credential.unwrap();
    assert_eq!(credential.username.as_deref(), Some("task-runtime"));
    assert_eq!(credential.password.as_deref(), Some("runtime-fixture"));
    assert_eq!(credential.source.as_deref(), Some("isolated_task"));
    assert_eq!(options.repl_set_name.as_deref(), Some("fixture"));
    assert_eq!(options.direct_connection, Some(true));
}

#[tokio::test]
async fn auth_source_replacement_is_case_insensitive() {
    let uri = runtime_uri(
        "mongodb://127.0.0.1:27017/?authSource=admin&AUTHSOURCE=other&appName=fixture%20probe+tag",
        "ticket19-runtime",
        "runtime-fixture",
        "isolated_task",
    );
    let options = mongodb::options::ClientOptions::parse(uri.as_str())
        .await
        .unwrap();
    assert_eq!(options.app_name.as_deref(), Some("fixture probe+tag"));
    assert_eq!(
        options.credential.unwrap().source.as_deref(),
        Some("isolated_task")
    );
    assert_eq!(
        uri.query_pairs()
            .filter(|(key, _)| key.eq_ignore_ascii_case("authSource"))
            .count(),
        1
    );
}

#[tokio::test]
async fn unauthenticated_bootstrap_preserves_path_and_repeated_options() {
    let uri = runtime_uri(
        "mongodb://127.0.0.1:27017/default_db?readPreference=secondary&readPreferenceTags=zone:east&readPreferenceTags=",
        "task-runtime",
        "runtime-fixture",
        "isolated_task",
    );
    let options = mongodb::options::ClientOptions::parse(uri.as_str())
        .await
        .unwrap();
    assert_eq!(options.default_database.as_deref(), Some("default_db"));
    assert_eq!(
        options.credential.unwrap().source.as_deref(),
        Some("isolated_task")
    );
    assert_eq!(
        uri.query_pairs()
            .filter(|(key, _)| key == "readPreferenceTags")
            .map(|(_, value)| value.into_owned())
            .collect::<Vec<_>>(),
        ["zone:east", ""]
    );
}

//! Offline import is a channel writer: while GitHub writes are disabled it may
//! not introduce a GitHub source, and it may still re-verify rows it stored.
use base64::{engine::general_purpose::STANDARD, Engine};
use cmaeg::records::{
    import::import_with, mongo, ImportGate, RecordError, GITHUB_IMPORT_DISABLED,
    GITHUB_IMPORT_KEYS_REQUIRED,
};
use cmaeg_core::{
    channel::{ChannelDefinition, Source},
    seal::{seal_blob, MasterKeyRing},
};
use mongodb::{
    bson::{doc, Document},
    Database,
};

fn keys() -> MasterKeyRing {
    MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([43; 32]))).unwrap()
}

fn row(id: &str, github: bool) -> String {
    let mut definition: ChannelDefinition = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    if github {
        definition.sources.push(Source::Github {
            app_ref: "pilot".into(),
            installation_ids: vec![7],
            repository_ids: vec![42],
            deny_repository_ids: Vec::new(),
            events: vec!["issues".into()],
            actions: vec!["opened".into()],
            prompt_template: None,
        });
    }
    let blob = seal_blob(&keys(), definition.to_blob_json().to_string().as_bytes()).unwrap();
    serde_json::json!({"record_id":id,"owner":"legacy-channel","version":"3","blob":STANDARD.encode(blob)})
        .to_string()
}

fn rejected(result: Result<u64, RecordError>) -> String {
    match result {
        Err(RecordError::Rejected(reason)) => reason,
        other => panic!("expected a rejected import, got {other:?}"),
    }
}

pub async fn verify(db: &Database) -> Result<(), Box<dyn std::error::Error>> {
    mongo::migrate(db).await?;
    let channels = db.collection::<Document>(mongo::RECORDS);
    let plain = uuid::Uuid::new_v4().to_string();
    let github = uuid::Uuid::new_v4().to_string();
    let path = std::env::temp_dir().join(format!("cma-eg-github-import-{}", uuid::Uuid::new_v4()));
    std::fs::write(
        &path,
        format!("{}\n{}\n", row(&plain, false), row(&github, true)),
    )?;
    let disabled = ImportGate::new(false, Some(keys()));

    // Disabled: the GitHub row is refused before any write; earlier rows stand.
    assert_eq!(
        rejected(import_with(db, &path, &disabled).await),
        GITHUB_IMPORT_DISABLED
    );
    assert!(channels.find_one(doc! {"_id":&plain}).await?.is_some());
    assert!(channels.find_one(doc! {"_id":&github}).await?.is_none());

    // Disabled without a key ring: no channel row can be inspected, so none is created.
    let unkeyed =
        std::env::temp_dir().join(format!("cma-eg-github-import-{}", uuid::Uuid::new_v4()));
    let fresh = uuid::Uuid::new_v4().to_string();
    std::fs::write(&unkeyed, row(&fresh, false))?;
    assert_eq!(
        rejected(import_with(db, &unkeyed, &ImportGate::new(false, None)).await),
        GITHUB_IMPORT_KEYS_REQUIRED
    );
    assert!(channels.find_one(doc! {"_id":&fresh}).await?.is_none());
    std::fs::remove_file(unkeyed)?;

    // Enabled: the same export imports both rows.
    assert_eq!(
        import_with(db, &path, &ImportGate::new(true, None)).await?,
        2
    );
    assert!(channels.find_one(doc! {"_id":&github}).await?.is_some());

    // Disabled again: rows already stored unchanged are retained, even unkeyed.
    assert_eq!(import_with(db, &path, &disabled).await?, 2);
    assert_eq!(
        import_with(db, &path, &ImportGate::new(false, None)).await?,
        2
    );
    std::fs::remove_file(path)?;
    Ok(())
}

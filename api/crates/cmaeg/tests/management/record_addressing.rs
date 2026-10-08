use super::*;
use cmaeg::{
    auth::Creator,
    channels::{management, ChannelTarget, ChannelUpdateRequest},
};
use cmaeg_core::{
    ids::{ChannelId, RecordId},
    seal::seal_blob,
};

fn owner(subject: &str) -> Creator {
    Creator {
        subject: subject.into(),
        token_hash: "fixture-owner-hash".into(),
        organization: false,
    }
}

struct App {
    state: ManagementState,
    provider: FakeProvider,
    cma: FakeCma,
    nyxid: FakeNyxid,
    created: management::CreatedChannel,
}

impl App {
    async fn new(reserved: bool) -> Self {
        let provider = FakeProvider::start(Default::default()).await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let state = management_state(&provider, &cma, &nyxid, key_ring(1));
        let mut body = create_body();
        if reserved {
            body["record_id"] = uuid::Uuid::new_v4().to_string().into();
        }
        let created = management::create(
            &state,
            &owner("alice"),
            serde_json::from_value(body).unwrap(),
        )
        .await
        .unwrap();
        Self {
            state,
            provider,
            cma,
            nyxid,
            created,
        }
    }
    fn target(&self) -> ChannelTarget {
        ChannelTarget::Record(self.created.record_id.clone())
    }
    async fn close(self) {
        self.provider.close().await;
        self.cma.close().await;
        self.nyxid.close().await;
    }
}

#[tokio::test]
async fn record_addressing_enforces_ownership_and_preserves_supplied_ids() {
    let app = App::new(false).await;
    let by_id = management::load(
        &app.state,
        &owner("alice"),
        &ChannelTarget::Id(app.created.channel_id.clone()),
    )
    .await
    .unwrap();
    let by_record = management::load(&app.state, &owner("alice"), &app.target())
        .await
        .unwrap();
    assert_eq!(by_id.definition, by_record.definition);
    assert_eq!(by_id.record, by_record.record);
    assert_eq!(
        by_record
            .channel_id
            .open(&app.state.master_keys)
            .unwrap()
            .payload,
        by_id.pointer
    );
    let resolved = app
        .state
        .resolver
        .resolve(by_record.channel_id.as_str())
        .await
        .unwrap();
    assert_eq!(resolved.version, by_record.record.version);
    // Serving authority takes its version from the record store, not the
    // previous write's version retained inside the encrypted definition.
    let mut authoritative = by_record.definition.clone();
    authoritative.version = by_record.record.version;
    assert_eq!(resolved.definition, authoritative);
    let alias = by_id.pointer.seal_id(&app.state.master_keys).unwrap();
    let loaded = management::load(
        &app.state,
        &owner("alice"),
        &ChannelTarget::Id(alias.clone()),
    )
    .await
    .unwrap();
    assert_eq!(loaded.channel_id.as_str(), alias.as_str());
    assert!(matches!(
        management::load(&app.state, &owner("bob"), &app.target()).await,
        Err(management::ManagementError::NotFound)
    ));
    assert_eq!(app.cma.records()[app.created.record_id.as_str()].version, 1);
    app.close().await;
}

#[tokio::test]
async fn every_existing_management_mutation_accepts_a_record_target() {
    let app = App::new(true).await;
    let loaded = management::load(&app.state, &owner("alice"), &app.target())
        .await
        .unwrap();
    assert_eq!(loaded.channel_id, app.created.channel_id);
    let mut body = create_body();
    body["name"] = "record-addressed-update".into();
    body["expected_version"] = loaded.record.version.into();
    let request: ChannelUpdateRequest = serde_json::from_value(body).unwrap();
    let changed = management::update(&app.state, &owner("alice"), &app.target(), request)
        .await
        .unwrap();
    assert_eq!(changed.definition.name, "record-addressed-update");
    assert_eq!(changed.channel_id, app.created.channel_id);
    assert_eq!(
        management::rekey(&app.state, &app.target()).await.unwrap(),
        1
    );
    assert_eq!(
        management::test_turn(&app.state, &owner("alice"), &app.target())
            .await
            .unwrap()
            .first_text,
        "hello"
    );
    let version = management::load(&app.state, &owner("alice"), &app.target())
        .await
        .unwrap()
        .record
        .version;
    management::delete(&app.state, &owner("alice"), &app.target(), version)
        .await
        .unwrap();
    assert!(app.cma.records().is_empty());
    app.close().await;
}

fn invalid_pointer(error: management::ManagementError) {
    let management::ManagementError::Invalid(error) = error else {
        panic!("expected invalid pointer")
    };
    assert_eq!(error.pointer, "/channel_id");
    assert_eq!(error.message, "malformed channel pointer");
}

#[tokio::test]
async fn record_mutations_clear_a_negative_entry_for_the_retained_identity() {
    use cmaeg::channels::resolver::ResolveError;
    for operation in ["update", "rekey", "delete"] {
        let app = App::new(true).await;
        let record = app.cma.records()[app.created.record_id.as_str()].clone();
        app.state
            .records
            .delete(&app.created.record_id, record.version)
            .await
            .unwrap();
        assert!(matches!(
            app.state
                .resolver
                .resolve(app.created.channel_id.as_str())
                .await,
            Err(ResolveError::NotFound)
        ));
        assert_eq!(app.state.resolver.cache_len(), 1);
        app.cma
            .seed(app.created.record_id.as_str(), "alice", record.blob);
        match operation {
            "update" => {
                let mut body = create_body();
                body["expected_version"] = 1.into();
                management::update(
                    &app.state,
                    &owner("alice"),
                    &app.target(),
                    serde_json::from_value(body).unwrap(),
                )
                .await
                .unwrap();
            }
            "rekey" => {
                management::rekey(&app.state, &app.target()).await.unwrap();
            }
            _ => {
                management::delete(&app.state, &owner("alice"), &app.target(), 1)
                    .await
                    .unwrap();
            }
        }
        assert_eq!(app.state.resolver.cache_len(), 0);
        let resolved = app
            .state
            .resolver
            .resolve(app.created.channel_id.as_str())
            .await;
        if operation == "delete" {
            assert!(matches!(resolved, Err(ResolveError::NotFound)));
            assert!(app.cma.records().is_empty());
        } else {
            assert_eq!(resolved.unwrap().version, 2);
        }
        app.close().await;
    }
}

#[tokio::test]
async fn malformed_record_targets_are_refused_before_a_record_fetch() {
    let app = App::new(false).await;
    let before = app.cma.gets.load(std::sync::atomic::Ordering::SeqCst);
    for value in [
        "not-a-uuid",
        "../other",
        "00000000-0000-0000-0000-000000000000",
        "record?query=1",
    ] {
        let target = ChannelTarget::Record(RecordId::new(value).unwrap());
        invalid_pointer(
            management::load(&app.state, &owner("alice"), &target)
                .await
                .unwrap_err(),
        );
    }
    assert_eq!(
        app.cma.gets.load(std::sync::atomic::Ordering::SeqCst),
        before
    );
    app.close().await;
}

#[tokio::test]
async fn corrupt_retained_identities_never_fall_back_to_new_ids() {
    for invalid in ["malformed", "unopenable", "mismatched", "missing-connected"] {
        let app = App::new(true).await;
        let loaded = management::load(&app.state, &owner("alice"), &app.target())
            .await
            .unwrap();
        let mut definition = loaded.definition;
        match invalid {
            "malformed" => {
                definition.creation.as_mut().unwrap().channel_id =
                    ChannelId::new("cmaeg1.ch.malformed").unwrap()
            }
            "unopenable" => {
                let other_ring = cmaeg_core::seal::MasterKeyRing::parse(&format!(
                    "1:{}",
                    STANDARD_NO_PAD.encode([9; 32])
                ))
                .unwrap();
                definition.creation.as_mut().unwrap().channel_id =
                    loaded.pointer.seal_id(&other_ring).unwrap();
            }
            "mismatched" => {
                let mut pointer = loaded.pointer.clone();
                pointer.record_url = pointer.record_url.replace("127.0.0.1", "localhost");
                definition.creation.as_mut().unwrap().channel_id =
                    pointer.seal_id(&app.state.master_keys).unwrap();
            }
            _ => {
                definition.creation = None;
                definition.connection_ref = Some(uuid::Uuid::new_v4().to_string());
            }
        }
        let blob = seal_blob(
            &app.state.master_keys,
            definition.to_blob_json().to_string().as_bytes(),
        )
        .unwrap();
        app.state
            .records
            .put(
                &definition.record_id,
                "alice",
                &blob,
                Some(loaded.record.version),
            )
            .await
            .unwrap();
        let stored = app.cma.records()[definition.record_id.as_str()].clone();
        assert!(matches!(
            management::load(&app.state, &owner("bob"), &app.target()).await,
            Err(management::ManagementError::NotFound)
        ));
        let after = app.cma.records()[definition.record_id.as_str()].clone();
        assert_eq!(after.version, stored.version);
        assert_eq!(after.blob, stored.blob);
        invalid_pointer(
            management::load(&app.state, &owner("alice"), &app.target())
                .await
                .unwrap_err(),
        );
        app.close().await;
    }
}

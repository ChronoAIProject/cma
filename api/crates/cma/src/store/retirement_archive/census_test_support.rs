use super::*;
use futures_util::TryStreamExt;
use std::collections::BTreeMap;

pub(in crate::store) async fn prepare_source(
    client: Client,
    database: Database,
    id: &str,
) -> (Temp, ArchiveStore, Request, Vec<u8>) {
    let temp = Temp::new();
    let mut request = request(&temp);
    let mut source = database
        .collection::<Document>("sandboxes")
        .find_one(doc! {"_id":id})
        .await
        .unwrap()
        .unwrap();
    let mut parent = read_sandbox(&source).unwrap();
    if parent.status == SandboxStatus::Reviving {
        parent.begin_retirement(layer2_core::now_rfc3339()).unwrap();
    } else {
        parent.status = SandboxStatus::Retiring;
        parent.work.step = SandboxWorkStep::StopAgents;
        parent.work.lease = None;
    }
    let mut home = SandboxHome::reserve(
        &parent.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture".into(),
            capacity_gib: 64,
            template_name: "seed".into(),
            template_generation: "generation".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "volume".into(),
        storage_generation: "generation".into(),
    })
    .unwrap();
    request.home_identity = home.attachment().unwrap().identity().unwrap();
    parent.lifecycle.home = Some(Box::new(home));
    parent.validate().unwrap();
    let seq = source.get_i64("last_seq").unwrap() as u64 + 1;
    source.extend(crate::store::sandboxes::sandbox_document(&parent, seq).unwrap());
    let intent = crate::store::sandbox_events::event_document(&SandboxEvent {
        sandbox_id: parent.id.clone(),
        seq,
        appended_at: layer2_core::now_rfc3339(),
        kind: SandboxEventKind::StatusChanged {
            from: SandboxStatus::Active,
            to: SandboxStatus::Retiring,
        },
    })
    .unwrap();
    request.sandbox_id = id.into();
    request.expected_revision = parent.revision;
    request.source_sha256 = digest(&mongodb::bson::serialize_to_vec(&source).unwrap());
    request.retirement_intent_sha256 = digest(&mongodb::bson::serialize_to_vec(&intent).unwrap());
    qualify(&temp, &mut request);
    database
        .collection::<Document>("sandboxes")
        .replace_one(doc! {"_id":id}, source)
        .await
        .unwrap();
    database
        .collection::<Document>("sandbox_events")
        .insert_one(intent)
        .await
        .unwrap();
    let bytes = serde_json::to_vec(&request).unwrap();
    (temp, ArchiveStore { client, database }, request, bytes)
}

// This inventory reads entire fixture collections, never the archive selector.
// Only the independently specified owner-wide budget baseline is excluded.
pub(super) async fn inventory(db: &Database, names: &[&str]) -> BTreeMap<String, Vec<u8>> {
    let mut expected = BTreeMap::new();
    for name in names {
        let rows = db
            .collection::<mongodb::bson::RawDocumentBuf>(name)
            .find(doc! {})
            .sort(doc! {"_id":1})
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap();
        let mut bytes = Vec::new();
        for row in rows {
            if *name == "model_budget_reservations"
                && row.get_str("_id").unwrap().starts_with("owner:")
            {
                continue;
            }
            bytes.extend(row.as_bytes());
        }
        assert!(!bytes.is_empty(), "store fixture must populate {name}");
        expected.insert((*name).into(), bytes);
    }
    expected
}

pub(super) async fn roundtrip(
    fixture: (Temp, ArchiveStore, Request, Vec<u8>),
    expected: BTreeMap<String, Vec<u8>>,
) {
    roundtrip_with_blobs(fixture, expected, None).await;
}

pub(super) async fn roundtrip_with_blobs(
    fixture: (Temp, ArchiveStore, Request, Vec<u8>),
    expected: BTreeMap<String, Vec<u8>>,
    blobs: Option<&dyn layer2_core::document_artifacts::DocumentBlobStore>,
) {
    let (temp, store, request, bytes) = fixture;
    let custody = Custody::default();
    store.admit(&request, &bytes).await.unwrap();
    let manifest = export::publish(&store, &request, &bytes, &custody, blobs)
        .await
        .unwrap();
    let restored = verify::restore(
        &custody,
        &manifest,
        &request,
        &bytes,
        &temp.0.join("restore"),
    )
    .await
    .unwrap();
    for (name, data) in &expected {
        let actual =
            std::fs::read(temp.0.join("restore/database").join(format!("{name}.bson"))).unwrap();
        assert_eq!(
            actual.len(),
            data.len(),
            "independent census length for {name}"
        );
        assert_eq!(
            digest(&actual),
            digest(data),
            "independent census bytes for {name}"
        );
    }
    let receipt = custody
        .retain(
            &request.operation_id,
            serde_json::to_vec(&restored).unwrap(),
        )
        .await
        .unwrap();
    store
        .retain_restore(&request, &bytes, &receipt)
        .await
        .unwrap();
    store
        .complete(
            &request,
            &bytes,
            RetirementArchiveReceipt {
                manifest_sha256: manifest.sha256.clone(),
                custody_sha256: restored.custody_sha256,
                verification_sha256: receipt.sha256,
            },
        )
        .await
        .unwrap();
    store.database.drop().await.unwrap();
    verify::restore(
        &custody,
        &manifest,
        &request,
        &bytes,
        &temp.0.join("source-independent"),
    )
    .await
    .unwrap();
}

//! Test boundaries interrupt actual persistence between durable phases.
use super::super::projection;
use super::*;
use layer2_core::sandbox::AgentLogEvidenceChange;

pub(in crate::store) async fn claim(
    client: &Client,
    db: &Database,
    agent: &str,
    audit: &str,
) -> Result<Document, SandboxStoreError> {
    progress::claim(client, db, agent, audit, false)
        .await?
        .document()
}
fn decode(row: Document) -> Result<Progress, SandboxStoreError> {
    let agent = row.get_str("agent_id").map_err(|_| corrupt())?.to_owned();
    let audit = row.get_str("audit_id").map_err(|_| corrupt())?.to_owned();
    progress::read(row, &agent, &audit)
}
pub(in crate::store) async fn collect(
    client: &Client,
    db: &Database,
    row: Document,
) -> Result<Document, SandboxStoreError> {
    let mut progress = decode(row)?;
    collect::run(client, db, &mut progress).await?;
    progress.document()
}
pub(in crate::store) async fn promote(
    client: &Client,
    db: &Database,
    row: Document,
) -> Result<(Document, bool), SandboxStoreError> {
    let mut progress = decode(row)?;
    let done = persistence::promote_batch(client, db, &mut progress).await?;
    Ok((progress.document()?, done))
}
pub(in crate::store) async fn partial(
    client: &Client,
    db: &Database,
    row: Document,
    change: AgentLogEvidenceChange,
) -> Result<Document, SandboxStoreError> {
    let mut progress = decode(row)?;
    let generation = format!("audit-stage:{}", uuid::Uuid::new_v4().simple());
    progress.stage = Some(Stage::Collecting {
        generation: generation.clone(),
    });
    let evidence = projection::plan(&progress.head(&generation), change)?;
    let mut batch = persistence::Batch::default();
    assert!(batch.push(&evidence)?);
    persistence::write(client, db, &mut progress, &batch).await?;
    progress.document()
}
pub(in crate::store) async fn write_invalid(
    client: &Client,
    db: &Database,
    row: Document,
) -> Result<(), SandboxStoreError> {
    let mut progress = decode(row)?;
    let mut batch = persistence::Batch::default();
    assert!(batch.push(&doc! {"_id":"invalid-evidence-fixture"})?);
    persistence::write(client, db, &mut progress, &batch).await
}
pub(in crate::store) async fn touch(
    client: &Client,
    db: &Database,
    row: Document,
) -> Result<(), SandboxStoreError> {
    let mut progress = decode(row)?;
    persistence::write(client, db, &mut progress, &persistence::Batch::default()).await
}

#[test]
fn batches_bound_encoded_bytes_rows_and_replacement_growth() {
    let mut batch = persistence::Batch::default();
    for i in 0..persistence::MAX_ROWS {
        assert!(batch
            .push(&doc! {"_id":format!("row-{i}"),"value":"small"})
            .unwrap());
    }
    assert!(!batch.push(&doc! {"_id":"extra","value":"small"}).unwrap());
    assert!(batch
        .push(&doc! {"_id":"row-0","value":"replacement"})
        .unwrap());
    let mut batch = persistence::Batch::default();
    for i in 0..4 {
        assert!(batch
            .push(&doc! {"_id":format!("large-{i}"),"value":"x".repeat(250*1024)})
            .unwrap());
    }
    assert!(!batch
        .push(&doc! {"_id":"overflow","value":"x".repeat(250*1024)})
        .unwrap());
    assert!(batch.push(&doc! {"_id":"large-0","value":"small"}).unwrap());
    assert!(batch
        .push(&doc! {"_id":"reclaimed","value":"x".repeat(250*1024)})
        .unwrap());
    assert!(batch
        .push(&doc! {"_id":"oversize","value":"x".repeat(256*1024)})
        .is_err());
}

#[test]
fn batch_acknowledgment_requires_every_effect_and_no_embedded_write_error() {
    use super::persistence::require_complete_batch;
    for reply in [doc! {"n":2_i32}, doc! {"n":2_i64,"writeErrors":[]}] {
        require_complete_batch(&reply, 2).unwrap();
    }
    for reply in [
        doc! {},
        doc! {"n":1_i32},
        doc! {"n":3_i64},
        doc! {"n":2_f64},
        doc! {"n":2_i32,"writeErrors":[{"code":121}]},
        doc! {"n":2_i32,"writeErrors":true},
        doc! {"n":2_i32,"writeConcernError":{"code":64}},
    ] {
        assert!(require_complete_batch(&reply, 2).is_err());
    }
}

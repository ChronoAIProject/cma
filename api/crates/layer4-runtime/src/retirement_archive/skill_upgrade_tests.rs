use super::*;
use layer3_adapter::{AgentSkill, AgentSkillInstallReceipt, AgentSkillInstallRequest};

#[tokio::test]
async fn archive_preserves_upgrade_journal_and_authority_bytes_after_restart() {
    let fixture = Fixture::new();
    let (_, mut records, mut proof) = fixture.records("agent");
    let original = AgentSkillInstallRequest::new(
        AgentSlot::new("agent"),
        AgentSkill {
            id: "research".into(),
            name: None,
            version: Some("1.0".into()),
            archive_sha256: "a".repeat(64),
            source_url: "https://example.invalid/archive".into(),
        },
    );
    let mut next = original.skill.clone();
    next.version = Some("1.1".into());
    next.archive_sha256 = "b".repeat(64);
    let upgrade = AgentSkillInstallRequest::upgrade(
        original.agent_id.clone(),
        next,
        original.skill.clone(),
        Some(AgentSkillInstallReceipt::from_request(&original)),
        2,
    );
    for (index, request) in [&original, &upgrade].into_iter().enumerate() {
        let operation = Operation::install_agent_skill(request);
        let journal = Journal::started(&fixture.config, &operation)
            .unwrap()
            .completed(&AgentSkillInstallReceipt::from_request(request))
            .unwrap();
        let authority = Authority::new(&fixture.config, index as u64 + 1, &operation);
        for (kind, path, bytes) in [
            (
                1,
                journal.inventory_name(&fixture.config),
                journal.encode().unwrap(),
            ),
            (
                2,
                authority.inventory_name(&fixture.config),
                serde_json::to_vec(&authority).unwrap(),
            ),
        ] {
            records.push(SourceRecord {
                scope: 0,
                kind,
                name: path.file_name().unwrap().to_str().unwrap().into(),
                bytes,
            });
        }
    }
    records.sort_by(|a, b| (a.scope, a.kind, &a.name).cmp(&(b.scope, b.kind, &b.name)));
    let mut digest = Sha256::new();
    for record in &records {
        record.hash_into(&mut digest);
    }
    proof.source_sha256 = format!("{:x}", digest.finalize());
    proof.records = records.len() as u32;
    let expected = records.iter().map(|r| r.bytes.clone()).collect::<Vec<_>>();
    let archive = fixture.open(true).await;
    archive
        .prepare(proof.clone(), move |index| {
            Ok(records[index as usize].clone())
        })
        .await
        .unwrap();
    archive
        .complete(proof.key, proof.source_sha256)
        .await
        .unwrap();
    archive.close().await.unwrap();
    let archive = fixture.open(false).await;
    assert_eq!(
        archive
            .authorize(&[Operation::install_agent_skill(&upgrade)])
            .await
            .unwrap_err()
            .code,
        "mutation_resource_retired"
    );
    archive.close().await.unwrap();
    let db = rusqlite::Connection::open(fixture.path.join("ledger.sqlite3")).unwrap();
    let retained = db
        .prepare("SELECT payload FROM archived_records ORDER BY scope,kind,source_name")
        .unwrap()
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(retained, expected);
}

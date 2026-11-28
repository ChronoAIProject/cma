use super::*;
use layer2_core::sandbox::agent_output_index::{AgentOutputChange, AgentOutputTotals};

fn output(ordinal: u64) -> Document {
    let change = AgentOutputChange {
        turn_sequence: 1,
        ordinal,
        source_sequence: ordinal + 2,
        before: AgentOutputTotals {
            items: ordinal,
            messages: ordinal,
            nonempty_messages: ordinal,
            tool_starts: 0,
            tool_finishes: 0,
            text: Default::default(),
        },
        after: AgentOutputTotals {
            items: ordinal + 1,
            messages: ordinal + 1,
            nonempty_messages: ordinal + 1,
            tool_starts: 0,
            tool_finishes: 0,
            text: Default::default(),
        },
        item_ordinal: ordinal,
        message_ordinal: Some(ordinal),
        tool: None,
    };
    doc! {"_id":format!("output-{ordinal}"),"format_version":1_i64,
    "agent_id":"agt_fixture","incarnation":"a".repeat(64),
    "evidence_generation":"fixture-generation","kind":"output_change",
    "entity_key":format!("1:{ordinal}"),"through_seq":30_i64,"head_revision":1_i64,
    "record":{"type":"output_change","change":bson::serialize_to_document(&change).unwrap()},
    "turn_sequence":1_i64,"change_ordinal":ordinal as i64}
}

#[tokio::test]
#[ignore = "requires a disposable replica set"]
async fn output_seek_is_numeric_unique_partial_and_matches_the_typed_record() {
    let (client, name) = database("output_seek").await;
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let rows = db.collection::<Document>(EVIDENCE);
    for ordinal in [10, 2, 0] {
        rows.insert_one(output(ordinal)).await.unwrap();
    }
    let result: Vec<Document> = rows
        .find(doc! {"agent_id":"agt_fixture",
        "incarnation":"a".repeat(64),"evidence_generation":"fixture-generation",
        "kind":"output_change","turn_sequence":1_i64,"change_ordinal":{"$gt":0_i64}})
        .sort(doc! {"change_ordinal":1})
        .hint(mongodb::options::Hint::Name(
            "agent_log_output_change_seek".into(),
        ))
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(
        result
            .iter()
            .map(|r| r.get_i64("change_ordinal").unwrap())
            .collect::<Vec<_>>(),
        vec![2, 10]
    );
    let mut duplicate = output(2);
    duplicate.insert("_id", "alias");
    duplicate.insert("entity_key", "alias");
    assert!(crate::store::sandbox_mongo::is_duplicate_key(
        &rows.insert_one(duplicate).await.unwrap_err()
    ));
    for case in 0..8 {
        let mut bad = output(3);
        match case {
            0 => {
                bad.remove("turn_sequence");
            }
            1 => {
                bad.insert("change_ordinal", 3_i32);
            }
            2 => {
                bad.insert("turn_sequence", MAX_SEQUENCE + 1);
            }
            3 => {
                bad.insert("change_ordinal", -1_i64);
            }
            4 => {
                bad.insert("change_ordinal", 4_i64);
            }
            5 => {
                bad.get_document_mut("record")
                    .unwrap()
                    .get_document_mut("change")
                    .unwrap()
                    .insert("ordinal", 3_i32);
            }
            6 => {
                bad.get_document_mut("record")
                    .unwrap()
                    .insert("type", "message");
            }
            _ => {
                bad.insert("kind", "message");
            }
        }
        let error = rows.insert_one(bad).await.unwrap_err();
        assert!(
            matches!(*error.kind,
            mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(ref e)) if e.code==121),
            "case {case}: {error}"
        );
    }
    // Two non-output records need no seek fields and do not collide in the partial index.
    for suffix in ["a", "b"] {
        let mut other = output(3);
        other.insert("_id", suffix);
        other.insert("kind", "message");
        other.insert("entity_key", suffix);
        other.insert(
            "record",
            doc! {"type":"message","sequence":3_i64,"turn_sequence":1_i64,"role":"assistant"},
        );
        other.remove("turn_sequence");
        other.remove("change_ordinal");
        rows.insert_one(other).await.unwrap();
    }
    assert_eq!(rows.count_documents(doc! {}).await.unwrap(), 5);
    schema::verify(&db).await.unwrap();
    // Same keys/name with a changed partial predicate must refuse before any DDL.
    rows.drop_index("agent_log_output_change_seek")
        .await
        .unwrap();
    rows.create_index(
        mongodb::IndexModel::builder()
            .keys(doc! {"agent_id":1,"incarnation":1,
            "evidence_generation":1,"kind":1,"turn_sequence":1,"change_ordinal":1})
            .options(
                mongodb::options::IndexOptions::builder()
                    .name("agent_log_output_change_seek".to_owned())
                    .unique(true)
                    .partial_filter_expression(doc! {"kind":"not_output"})
                    .build(),
            )
            .build(),
    )
    .await
    .unwrap();
    db.collection::<Document>(HEADS).drop().await.unwrap();
    assert!(schema::apply_next(&db).await.is_err());
    assert!(!db
        .list_collection_names()
        .await
        .unwrap()
        .iter()
        .any(|n| n == HEADS));
    db.drop().await.unwrap();
}

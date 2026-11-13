use super::*;

#[test]
fn stage_batches_bound_bytes_and_rows_and_coalesce_repeated_facts() {
    let mut batch = Batch::default();
    for index in 0..BATCH_ROWS {
        batch.insert(doc! {"_id":index.to_string()}, 100).unwrap();
    }
    assert!(!batch.fits(&doc! {"_id":"new"}, 100).unwrap());
    batch.insert(doc! {"_id":"0","new":true}, 200).unwrap();
    assert_eq!(batch.rows.len(), BATCH_ROWS);
    assert_eq!(batch.bytes, BATCH_ROWS * 100 + 100);
    assert!(batch.rows["0"].0.get_bool("new").unwrap());

    let mut batch = Batch::default();
    for index in 0..4 {
        batch
            .insert(doc! {"_id":index.to_string()}, RECORD_BYTES)
            .unwrap();
    }
    assert!(!batch.fits(&doc! {"_id":"overflow"}, 1).unwrap());
    batch.insert(doc! {"_id":"0"}, 1).unwrap();
    assert!(batch.fits(&doc! {"_id":"fits"}, RECORD_BYTES - 1).unwrap());
    assert!(!batch
        .fits(&doc! {"_id":"oversize"}, RECORD_BYTES + 1)
        .unwrap());
    assert!(batch
        .insert(doc! {"_id":"oversize"}, RECORD_BYTES + 1)
        .is_err());
}

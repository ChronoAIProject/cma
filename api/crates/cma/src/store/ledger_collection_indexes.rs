//! Read-only, bounded index qualification. Never trust a key-pattern hint alone.
use mongodb::{
    bson::{doc, Bson, Document},
    Database,
};

use super::super::{CollectionReadRefusal::AuthorityUnavailable, Result};

pub(super) async fn read(database: &Database, collection: &str) -> Result<Vec<Document>> {
    let response = database
        .run_command(doc! {"listIndexes":collection,"cursor":{"batchSize":64},"maxTimeMS":2000})
        .await
        .map_err(|_| AuthorityUnavailable)?;
    let cursor = response
        .get_document("cursor")
        .map_err(|_| AuthorityUnavailable)?;
    let id = cursor.get_i64("id").map_err(|_| AuthorityUnavailable)?;
    if id != 0 {
        // Do not leave a server cursor behind or fetch an unbounded inventory.
        let _ = database
            .run_command(doc! {"killCursors":collection,"cursors":[id],"maxTimeMS":2000})
            .await;
        return Err(AuthorityUnavailable);
    }
    let batch = cursor
        .get_array("firstBatch")
        .map_err(|_| AuthorityUnavailable)?;
    if batch.is_empty() || batch.len() > 64 {
        return Err(AuthorityUnavailable);
    }
    batch
        .iter()
        .map(|value| value.as_document().cloned().ok_or(AuthorityUnavailable))
        .collect()
}

pub(super) fn complete_name(inventory: &[Document], keys: &Document) -> Result<String> {
    let mut matching = inventory.iter().filter(|index| {
        index
            .get_document("key")
            .is_ok_and(|key| key.iter().eq(keys.iter()))
    });
    let index = matching.next().ok_or(AuthorityUnavailable)?;
    if matching.next().is_some()
        || ["sparse", "hidden"]
            .iter()
            .any(|field| !matches!(index.get(*field), None | Some(Bson::Boolean(false))))
        || !matches!(index.get("unique"), None | Some(Bson::Boolean(_)))
        || index.keys().any(|field| {
            ![
                "v",
                "ns",
                "name",
                "key",
                "unique",
                "sparse",
                "hidden",
                "background",
            ]
            .contains(&field.as_str())
        })
    {
        return Err(AuthorityUnavailable);
    }
    let name = index.get_str("name").map_err(|_| AuthorityUnavailable)?;
    if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
        return Err(AuthorityUnavailable);
    }
    Ok(name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Document {
        doc! {"v":2,"name":"full_fixture","key":{"worktree_id":1,"_id":1}}
    }

    #[test]
    fn complete_indexes_allow_only_explicit_safe_options() {
        let keys = doc! {"worktree_id":1,"_id":1};
        let mut index = full();
        assert_eq!(
            complete_name(&[index.clone()], &keys).unwrap(),
            "full_fixture"
        );
        index.extend(doc! {"unique":true,"sparse":false,"hidden":false,"background":true});
        assert!(complete_name(&[index], &keys).is_ok());
        assert_eq!(
            complete_name(
                &[doc! {"v":2,"name":"_id_","key":{"_id":1}}],
                &doc! {"_id":1}
            )
            .unwrap(),
            "_id_"
        );
    }

    #[test]
    fn incomplete_ambiguous_or_reordered_indexes_refuse() {
        let keys = doc! {"worktree_id":1,"_id":1};
        for (field, value) in [
            (
                "partialFilterExpression",
                Bson::Document(doc! {"state":"deleted"}),
            ),
            ("sparse", Bson::Boolean(true)),
            ("hidden", Bson::Boolean(true)),
            ("collation", Bson::Document(doc! {"locale":"en"})),
            ("expireAfterSeconds", Bson::Int32(60)),
            ("unique", Bson::Null),
            ("sparse", Bson::Null),
            ("unreviewedOption", Bson::Boolean(false)),
            ("name", Bson::String(String::new())),
        ] {
            let mut index = full();
            index.insert(field, value);
            assert!(complete_name(&[index], &keys).is_err(), "{field}");
        }
        assert!(complete_name(&[], &keys).is_err());
        assert!(complete_name(&[full(), full()], &keys).is_err());
        assert!(complete_name(&[full()], &doc! {"_id":1,"worktree_id":1}).is_err());
    }
}

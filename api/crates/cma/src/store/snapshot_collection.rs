//! Explicit read concern for snapshot sessions, preserving collection policy.

use mongodb::{
    bson::Document,
    options::{CollectionOptions, ReadConcern},
    Collection,
};

pub(super) fn collection(source: &Collection<Document>, snapshot: bool) -> Collection<Document> {
    if snapshot {
        source
            .client()
            .database(&source.namespace().db)
            .collection_with_options(
                source.name(),
                CollectionOptions::builder()
                    .read_concern(ReadConcern::snapshot())
                    .write_concern(source.write_concern().cloned())
                    .selection_criteria(source.selection_criteria().cloned())
                    .build(),
            )
    } else {
        source.clone()
    }
}

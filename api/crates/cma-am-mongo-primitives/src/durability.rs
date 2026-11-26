//! Mongo collection defaults required by authority-bearing records.

use mongodb::options::{CollectionOptions, ReadConcern, WriteConcern};
use mongodb::{Collection, Database};

/// Open a collection whose reads and writes use a replica-set majority.
///
/// Setting both concerns on the collection prevents authority decisions from
/// observing rollbackable local state and makes every write wait for durable
/// acknowledgement. Multi-record decisions still use a snapshot transaction.
pub fn majority_collection<T: Send + Sync>(database: &Database, name: &str) -> Collection<T> {
    database.collection_with_options(
        name,
        CollectionOptions::builder()
            .read_concern(ReadConcern::majority())
            .write_concern(WriteConcern::majority())
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use mongodb::options::{Acknowledgment, ReadConcernLevel};

    use super::*;

    #[tokio::test]
    async fn authority_collections_require_majority_reads_and_writes() {
        let client = mongodb::Client::with_uri_str("mongodb://127.0.0.1:27017")
            .await
            .expect("URI parses without opening a connection");
        let collection =
            majority_collection::<mongodb::bson::Document>(&client.database("test"), "authority");
        assert_eq!(
            collection.read_concern().map(|concern| &concern.level),
            Some(&ReadConcernLevel::Majority)
        );
        assert_eq!(
            collection
                .write_concern()
                .and_then(|concern| concern.w.as_ref()),
            Some(&Acknowledgment::Majority)
        );
    }
}

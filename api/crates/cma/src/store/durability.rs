//! Mongo collection defaults required by authority-bearing records.

use mongodb::options::{CollectionOptions, WriteConcern};
use mongodb::{Collection, Database};

/// Open a collection whose writes are acknowledged by a replica-set majority.
///
/// Setting this on the collection makes it apply equally to inserts, CAS
/// replacements, deletes, and find-and-modify operations. A caller adding a
/// new write cannot accidentally fall back to the driver's `w: 1` default.
pub(super) fn majority_collection<T: Send + Sync>(
    database: &Database,
    name: &str,
) -> Collection<T> {
    database.collection_with_options(
        name,
        CollectionOptions::builder()
            .write_concern(WriteConcern::majority())
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use mongodb::options::Acknowledgment;

    use super::*;

    #[tokio::test]
    async fn authority_collections_request_majority_acknowledgement() {
        let client = mongodb::Client::with_uri_str("mongodb://127.0.0.1:27017")
            .await
            .expect("URI parses without opening a connection");
        let collection =
            majority_collection::<mongodb::bson::Document>(&client.database("test"), "authority");
        assert_eq!(
            collection
                .write_concern()
                .and_then(|concern| concern.w.as_ref()),
            Some(&Acknowledgment::Majority)
        );
    }
}

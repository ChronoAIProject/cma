//! Make an injected ambiguous commit's outcome visible before a test reads it.
//!
//! A `failCommand` write-concern error returns the reply of a commit the
//! server has already applied, before that commit is majority-visible. Reads
//! issued at once can then observe neither half, or only part, of the
//! outcome (#3178). Writing one majority-acknowledged document orders after
//! every earlier commit, so once it is acknowledged each later snapshot or
//! majority read sees a single settled outcome. Call it after clearing the
//! failpoint and before asserting persisted state.

use mongodb::{bson::doc, options::WriteConcern, Client};

pub(crate) async fn majority_barrier(client: &Client, database: &str) {
    client
        .database(database)
        .collection::<mongodb::bson::Document>("test_visibility_barrier")
        .insert_one(doc! {})
        .write_concern(WriteConcern::majority())
        .await
        .expect("majority-acknowledged visibility barrier");
}

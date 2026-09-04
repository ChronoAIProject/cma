use mongodb::bson::doc;

use super::profiles::{profile_indexes, revision_indexes};

#[test]
fn required_profile_indexes_have_exact_keys() {
    let indexes = profile_indexes("profiles");
    assert_eq!(indexes[0].keys, doc! { "owner": 1, "updated_at": -1 });
    assert_eq!(
        indexes[1].keys,
        doc! { "visibility": 1, "kind": 1, "updated_at": -1 }
    );
    let revision = revision_indexes("revisions").remove(0);
    assert_eq!(revision.keys, doc! { "profile_id": 1, "revision": 1 });
    assert_eq!(revision.options.unwrap().unique, Some(true));
}

//! A single page is read once and then matched against the whole subject cohort.
use super::Error;
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Bson, Document},
    options::Hint,
    Database,
};

pub(super) async fn page(
    db: &Database,
    collection: &str,
    after: Option<&Bson>,
) -> Result<Vec<Document>, Error> {
    // $expr compares BSON types in sort order. A bare {_id:{$gt:after}}
    // would type-bracket and omit mixed legacy string/ObjectId identities.
    let filter = after.map_or_else(
        Document::new,
        |after| doc! {"$expr":{"$gt":["$_id",{"$literal":after}]}},
    );
    Ok(
        crate::store::durability::majority_collection::<Document>(db, collection)
            .find(filter)
            .sort(doc! {"_id":1})
            .hint(Hint::Name("_id_".into()))
            .limit(500)
            .await?
            .try_collect()
            .await?,
    )
}

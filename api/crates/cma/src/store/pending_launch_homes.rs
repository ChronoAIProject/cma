//! Bounded lookup of reserved Launch home receipts.
use layer2_core::StoreError;
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    Collection, IndexModel,
};

pub(super) const INDEX: &str = "sandbox_admissions_reserved_launch_homes";

pub(super) fn reserved_filter() -> Document {
    doc! {"_id.scope": "launch_home_capacity", "state": "reserved"}
}

pub(super) async fn ensure_index(admissions: &Collection<Document>) -> Result<(), StoreError> {
    admissions
        .create_index(
            IndexModel::builder()
                .keys(doc! {"state": 1, "_id": 1})
                .options(
                    IndexOptions::builder()
                        .name(INDEX.to_owned())
                        .partial_filter_expression(reserved_filter())
                        .build(),
                )
                .build(),
        )
        .await
        .map_err(|error| unavailable(&format!("create reserved Launch home index: {error}")))?;
    Ok(())
}

fn unavailable(message: &str) -> StoreError {
    StoreError::Unavailable(message.into())
}

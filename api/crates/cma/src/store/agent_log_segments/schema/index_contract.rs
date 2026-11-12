//! Compare the installed index semantics, including explicit nonunique indexes.
use mongodb::{
    bson,
    options::{Collation, IndexOptions},
    IndexModel,
};

pub(super) fn primary_matches(actual: &IndexModel) -> bool {
    let mut actual = actual.clone();
    let Some(options) = actual.options.as_mut() else {
        return false;
    };
    // Mongo omits the unique flag for its inherently unique primary index.
    options.unique = Some(options.unique.unwrap_or(true));
    matches(
        &actual,
        &IndexModel::builder()
            .keys(bson::doc! {"_id":1})
            .options(
                IndexOptions::builder()
                    .name("_id_".to_owned())
                    .unique(true)
                    .build(),
            )
            .build(),
    )
}

pub(super) fn matches(actual: &IndexModel, expected: &IndexModel) -> bool {
    let (Some(actual_options), Some(expected_options)) =
        (actual.options.as_ref(), expected.options.as_ref())
    else {
        return false;
    };
    actual.keys.iter().eq(expected.keys.iter())
        && actual_options.name == expected_options.name
        && actual_options.unique.unwrap_or(false) == expected_options.unique.unwrap_or(false)
        && actual_options.partial_filter_expression == expected_options.partial_filter_expression
        && actual_options.sparse.unwrap_or(false) == expected_options.sparse.unwrap_or(false)
        && actual_options.hidden.unwrap_or(false) == expected_options.hidden.unwrap_or(false)
        && actual_options.expire_after == expected_options.expire_after
        && collation_matches(
            actual_options.collation.as_ref(),
            expected_options.collation.as_ref(),
        )
}

fn collation_matches(actual: Option<&Collation>, expected: Option<&Collation>) -> bool {
    match (actual, expected) {
        (None, None) => true,
        (None, Some(expected)) => bson::serialize_to_document(expected)
            .is_ok_and(|value| value == bson::doc! {"locale":"simple"}),
        (Some(actual), Some(expected)) => {
            match (
                bson::serialize_to_document(actual),
                bson::serialize_to_document(expected),
            ) {
                (Ok(actual), Ok(expected)) => actual == expected,
                _ => false,
            }
        }
        (Some(_), None) => false,
    }
}

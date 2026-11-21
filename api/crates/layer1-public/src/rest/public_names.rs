//! Optional public identity enrichment after resource authorization has succeeded.

use std::{collections::BTreeMap, sync::Arc};

use layer2_core::{SubjectId, UserDirectory};

pub(super) async fn resolve(
    directory: &Option<Arc<UserDirectory>>,
    subjects: &[SubjectId],
) -> BTreeMap<String, String> {
    let Some(directory) = directory else {
        return BTreeMap::new();
    };
    match directory.display_names(subjects).await {
        Ok(names) => names,
        Err(_) => {
            tracing::warn!("public identity names unavailable for authorized resource read");
            BTreeMap::new()
        }
    }
}

#[cfg(test)]
#[path = "public_names_tests.rs"]
pub(crate) mod tests;

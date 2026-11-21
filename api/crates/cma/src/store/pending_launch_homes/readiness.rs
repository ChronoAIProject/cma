use super::*;
use crate::store::schema::index_contract_v1::{self, InspectionError};

pub(super) fn index_definition() -> Document {
    doc! {"name": INDEX, "key": {"state": 1, "_id": 1},
    "partialFilterExpression": reserved_filter()}
}

impl PendingLaunchHomes {
    pub(in crate::store) async fn readiness(&self) -> Result<(), PendingHomesObservation> {
        if !self.enabled {
            return Err(PendingHomesObservation::Disabled);
        }
        if !self.active() {
            return Err(PendingHomesObservation::CapacityInactive);
        }
        let database = self.client.database(&self.admissions.namespace().db);
        let indexes = match index_contract_v1::inspect_collection(&database, ADMISSIONS).await {
            Ok(Some(indexes)) => indexes,
            Ok(None) => return Err(PendingHomesObservation::IndexMissing),
            Err(InspectionError::DefinitionMismatch) => {
                return Err(PendingHomesObservation::IndexMismatch)
            }
            Err(InspectionError::Unavailable) => return Err(PendingHomesObservation::Unavailable),
        };
        match index_contract_v1::missing(&indexes, &index_definition()) {
            Ok(missing) if missing.is_empty() => Ok(()),
            Ok(_) => Err(PendingHomesObservation::IndexMissing),
            Err(_) => Err(PendingHomesObservation::IndexMismatch),
        }
    }

    pub(super) async fn require_ready(&self) -> Result<(), StoreError> {
        self.readiness()
            .await
            .map_err(|state| unavailable(&format!("pending home recovery is {state:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_index_contract_rejects_every_unreviewed_definition() {
        let desired = index_definition();
        assert!(
            index_contract_v1::missing(std::slice::from_ref(&desired), &desired)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            index_contract_v1::missing(&[], &desired).unwrap(),
            vec![desired.clone()]
        );
        for (key, value) in [
            ("name", mongodb::bson::bson!("another-name")),
            ("key", mongodb::bson::bson!({"_id":1,"state":1})),
            (
                "partialFilterExpression",
                mongodb::bson::bson!({"state":"reserved"}),
            ),
            ("hidden", mongodb::bson::bson!(true)),
            ("unique", mongodb::bson::bson!(true)),
            ("expireAfterSeconds", mongodb::bson::bson!(60)),
            ("collation", mongodb::bson::bson!({"locale":"en"})),
        ] {
            let mut drifted = desired.clone();
            drifted.insert(key, value);
            assert!(
                index_contract_v1::missing(&[drifted], &desired).is_err(),
                "{key}"
            );
        }
    }
}

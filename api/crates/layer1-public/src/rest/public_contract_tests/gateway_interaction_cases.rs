use super::Scenario;
use layer2_core::{
    gateway_interactions::{GatewayInteractionService, GatewayInteractionStore},
    Authorization, Capability, GatewayRecord, StoreError, SubjectId,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Records {
    values: Mutex<BTreeMap<String, (GatewayRecord, i64)>>,
    unavailable: bool,
}
impl Records {
    fn available(&self) -> Result<(), StoreError> {
        if self.unavailable {
            Err(StoreError::Unavailable("fixture unavailable".into()))
        } else {
            Ok(())
        }
    }
}
#[async_trait::async_trait]
impl GatewayInteractionStore for Records {
    async fn put(
        &self,
        id: &str,
        owner: &SubjectId,
        blob: Vec<u8>,
        expected: Option<u64>,
        expires_at: i64,
    ) -> Result<GatewayRecord, StoreError> {
        self.available()?;
        let mut values = self.values.lock().unwrap();
        if let Some(expected) = expected {
            let actual = values.get(id).map_or(0, |(r, _)| r.version);
            let (record, expiry) = values
                .get_mut(id)
                .ok_or(StoreError::RevisionConflict { expected, actual })?;
            if record.version != expected || record.owner != *owner || *expiry != expires_at {
                return Err(StoreError::RevisionConflict { expected, actual });
            }
            record.blob = blob;
            record.version += 1;
            return Ok(record.clone());
        }
        if let Some((record, _)) = values.get(id) {
            return Err(StoreError::RevisionConflict {
                expected: 0,
                actual: record.version,
            });
        }
        let record = GatewayRecord {
            record_id: id.into(),
            owner: owner.clone(),
            blob,
            version: 1,
            created_at: "now".into(),
            updated_at: "now".into(),
        };
        values.insert(id.into(), (record.clone(), expires_at));
        Ok(record)
    }
    async fn get(&self, id: &str) -> Result<Option<GatewayRecord>, StoreError> {
        self.available()?;
        Ok(self
            .values
            .lock()
            .unwrap()
            .get(id)
            .map(|(record, _)| record.clone()))
    }
    async fn delete(&self, id: &str, expected: u64) -> Result<bool, StoreError> {
        self.available()?;
        let mut values = self.values.lock().unwrap();
        let Some((record, _)) = values.get(id) else {
            return Ok(false);
        };
        if record.version != expected {
            return Err(StoreError::RevisionConflict {
                expected,
                actual: record.version,
            });
        }
        values.remove(id);
        Ok(true)
    }
}

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    let mode = name.strip_prefix("gateway_interactions_")?;
    let service = GatewayInteractionService::new(Arc::new(Records {
        unavailable: mode == "unavailable",
        ..Default::default()
    }));
    Some(Scenario {
        router: crate::rest::gateway_interaction_router(service),
        subject: (mode != "anonymous").then_some("gateway-owner"),
        authorization: if mode == "forbidden" {
            Authorization::none()
        } else {
            Authorization::new([Capability::ManageGatewayRecords])
        },
        symbols: BTreeMap::new(),
    })
}

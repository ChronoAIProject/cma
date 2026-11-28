#[cfg(test)]
use cma_am_triggers::launches::event_key;
pub(super) use cma_am_triggers::launches::{
    event_document, handoff_document, launch_document, read_event, read_handoff, read_launch,
    StoredLaunch,
};

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::ids::{LaunchId, ServiceHandoffId, ServiceId, SubjectId};
    use layer2_core::trigger_domain::{
        BrokerHandoffState, LaunchStoreError, ServiceConnectionHandoff,
    };

    #[test]
    fn event_keys_sort_by_sequence() {
        let id = LaunchId("lch_one".into());
        assert!(event_key(&id, 9) < event_key(&id, 10));
        assert!(event_key(&id, 10) < event_key(&id, 1_000));
    }

    #[test]
    fn handoff_query_fields_and_schema_are_strict() {
        let handoff = ServiceConnectionHandoff {
            id: ServiceHandoffId("hnd_one".into()),
            launch_id: LaunchId("lch_one".into()),
            owner: SubjectId::new("owner"),
            service_id: ServiceId::new("github"),
            intent_id: "intent-one".into(),
            hosted_url: "https://connection.example/one".into(),
            expires_at: "2026-09-05T01:00:00Z".into(),
            state: BrokerHandoffState::Pending,
            created_at: "2026-09-05T00:00:00Z".into(),
            updated_at: "2026-09-05T00:00:00Z".into(),
            external_id: "external-one".into(),
            revision: 1,
        };
        let document = handoff_document(&handoff).unwrap();
        assert_eq!(read_handoff(&document).unwrap(), handoff);

        let mut corrupt_owner = document.clone();
        corrupt_owner.insert("owner", "someone-else");
        assert!(matches!(
            read_handoff(&corrupt_owner),
            Err(LaunchStoreError::Corrupt(_))
        ));
        let mut future = document;
        future.insert("document_schema_version", 2_i64);
        assert!(matches!(
            read_handoff(&future),
            Err(LaunchStoreError::Corrupt(_))
        ));
    }
}

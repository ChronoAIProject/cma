use super::{DelegationDebt, ErasureInventory, KeyReceipt, ResidueReport};
use serde::Deserialize;
use std::collections::BTreeMap;

// Frozen C2 persisted field sets. These readers intentionally reject C3 fields.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct C2Inventory {
    sandboxes: Vec<String>,
    workspaces: Vec<String>,
    worktrees: Vec<String>,
    agents: Vec<String>,
    launches: Vec<String>,
    bindings: Vec<String>,
    triggers: Vec<String>,
    bots: Vec<String>,
    documents: Vec<String>,
    homes: Vec<String>,
    pending_key_ids: Vec<String>,
    counts: BTreeMap<String, u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct C2Residue {
    records: BTreeMap<String, u64>,
    objects: u64,
    homes: Vec<String>,
    keys: KeyReceipt,
}

#[test]
fn delegation_inventory_requires_a_c3_reader_only_when_debt_is_recorded() {
    let mut inventory = ErasureInventory::default();
    let old: C2Inventory =
        serde_json::from_value(serde_json::to_value(&inventory).unwrap()).unwrap();
    assert!(old.sandboxes.is_empty());
    assert!(old.workspaces.is_empty());
    assert!(old.worktrees.is_empty());
    assert!(old.agents.is_empty());
    assert!(old.launches.is_empty());
    assert!(old.bindings.is_empty());
    assert!(old.triggers.is_empty());
    assert!(old.bots.is_empty());
    assert!(old.documents.is_empty());
    assert!(old.homes.is_empty());
    assert!(old.pending_key_ids.is_empty());
    assert!(old.counts.is_empty());
    inventory.pending_delegations.push(DelegationDebt {
        bot_id: "organization-bot".into(),
        authorization_grant_id: "grant-a".into(),
        credential_incarnation: "incarnation-a".into(),
    });
    let stored = serde_json::to_value(&inventory).unwrap();
    let error = serde_json::from_value::<C2Inventory>(stored.clone()).unwrap_err();
    assert!(error
        .to_string()
        .contains("unknown field `pending_delegations`"));
    assert_eq!(
        serde_json::from_value::<ErasureInventory>(stored).unwrap(),
        inventory
    );
}

#[test]
fn delegation_settlement_receipt_requires_a_c3_reader() {
    let mut report = ResidueReport {
        keys: KeyReceipt {
            remaining: 0,
            empty_at: Some("2026-11-14T00:00:00Z".into()),
        },
        ..Default::default()
    };
    let old: C2Residue = serde_json::from_value(serde_json::to_value(&report).unwrap()).unwrap();
    assert!(old.records.is_empty());
    assert_eq!(old.objects, 0);
    assert!(old.homes.is_empty());
    assert_eq!(old.keys, report.keys);
    report.delegations = Some(report.keys.clone());
    let stored = serde_json::to_value(&report).unwrap();
    let error = serde_json::from_value::<C2Residue>(stored.clone()).unwrap_err();
    assert!(error.to_string().contains("unknown field `delegations`"));
    assert_eq!(
        serde_json::from_value::<ResidueReport>(stored).unwrap(),
        report
    );
}

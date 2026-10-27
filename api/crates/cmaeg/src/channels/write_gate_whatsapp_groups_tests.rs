use super::{check, definition, WriteFlags};
use cmaeg_core::channel::Source;
use serde_json::json;
#[test]
fn native_whatsapp_group_authoring_gate_allows_retention_and_removal_without_native_writes() {
    let original = definition(json!([crate::whatsapp::groups::tests::source_group()]));
    let enabled = WriteFlags {
        native_whatsapp: true,
        native_whatsapp_groups: true,
        ..Default::default()
    };
    assert!(check(None, &original, enabled).is_ok());
    assert!(check(
        None,
        &original,
        WriteFlags {
            native_whatsapp: true,
            ..Default::default()
        }
    )
    .is_err());
    assert!(check(Some(&original), &original, WriteFlags::default()).is_ok());
    let mut removed = original.clone();
    if let Source::Whatsapp { group_ids, .. } = &mut removed.sources[0] {
        group_ids.clear();
    }
    assert!(check(Some(&original), &removed, WriteFlags::default()).is_ok());
    let mut widened = original.clone();
    if let Source::Whatsapp { group_ids, .. } = &mut widened.sources[0] {
        group_ids.push("other".into());
    }
    assert!(check(
        Some(&original),
        &widened,
        WriteFlags {
            native_whatsapp: true,
            ..Default::default()
        }
    )
    .is_err());
    if let Source::Whatsapp {
        phone_number_id, ..
    } = &mut removed.sources[0]
    {
        *phone_number_id = "99999".into();
    }
    assert!(check(Some(&original), &removed, WriteFlags::default()).is_err());
}

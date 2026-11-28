//! Literal selectors shared by indexed batches and the multi-subject scanner.
use super::{
    catalog::{Erasure, InventorySet as Set, KeyRule, ReferenceRule as Ref},
    context::Context,
};
use mongodb::bson::{Bson, Document};

pub(super) fn strings<'a>(row: &'a Document, path: &str) -> Vec<&'a str> {
    fn visit<'a>(value: &'a Bson, parts: &[&str], out: &mut Vec<&'a str>) {
        match value {
            Bson::Array(values) => {
                for value in values {
                    visit(value, parts, out)
                }
            }
            Bson::Document(row) if !parts.is_empty() => {
                if let Some(value) = row.get(parts[0]) {
                    visit(value, &parts[1..], out)
                }
            }
            Bson::String(value) if parts.is_empty() => out.push(value),
            _ => {}
        }
    }
    let parts: Vec<_> = path.split('.').collect();
    let mut out = vec![];
    if let Some(value) = row.get(parts[0]) {
        visit(value, &parts[1..], &mut out)
    }
    out
}
pub(super) fn regex(value: &str) -> String {
    value
        .chars()
        .flat_map(|ch| {
            if ".^$*+?()[]{}|\\".contains(ch) {
                vec!['\\', ch]
            } else {
                vec![ch]
            }
        })
        .collect()
}
pub(super) fn keys(context: &Context, rule: KeyRule) -> Vec<String> {
    match rule {
        KeyRule::Subject => vec![context.subject.clone()],
        KeyRule::SubjectPrefix(prefix) => vec![format!("{prefix}{}", context.subject)],
        KeyRule::RequestLimit => {
            let hash = layer2_core::document_artifacts::digest(
                format!("subject:{}", context.subject).as_bytes(),
            );
            ["default", "create", "input"]
                .map(|tier| format!("{tier}:{hash}"))
                .into()
        }
        KeyRule::VisitorDay => vec![],
    }
}
pub(super) fn matches(rule: Erasure, row: &Document, context: &Context) -> bool {
    let has = |paths: &[&str], values: &[String]| {
        paths.iter().any(|path| {
            strings(row, path)
                .iter()
                .any(|id| values.iter().any(|value| value == id))
        })
    };
    match rule {
        Erasure::Owner(paths) => has(paths, std::slice::from_ref(&context.subject)),
        Erasure::Linked(set, paths) => has(paths, &context.ids(set)),
        Erasure::LinkedKey(set, prefix) => row.get_str("_id").is_ok_and(|key| {
            context
                .ids(set)
                .iter()
                .any(|id| key.starts_with(&format!("{prefix}{id}:")))
        }),
        Erasure::OwnerKey(KeyRule::VisitorDay) => row.get_str("_id").is_ok_and(|key| {
            key.split_once(':').is_some_and(|(day, subject)| {
                !day.is_empty() && day.parse::<u64>().is_ok() && subject == context.subject
            })
        }),
        Erasure::OwnerKey(key) => row
            .get_str("_id")
            .is_ok_and(|id| keys(context, key).iter().any(|key| key == id)),
        Erasure::ProvenAbsent(paths) => {
            let mut values = vec![context.subject.clone()];
            values.extend(context.sets.values().flatten().cloned());
            values.extend(context.shares.iter().cloned());
            has(paths, &values)
        }
        Erasure::ProvenAbsentJson(fields) => {
            let mut values = vec![context.subject.clone()];
            values.extend(context.sets.values().flatten().cloned());
            values.extend(context.shares.iter().cloned());
            values.extend(context.execution_digests.iter().cloned());
            fields.iter().any(|field| match row.get(*field) {
                None => false,
                Some(Bson::String(text)) => serde_json::from_str::<serde_json::Value>(text)
                    .map_or(true, |value| json_names(&value, &values)),
                Some(value @ (Bson::Document(_) | Bson::Array(_))) => bson_names(value, &values),
                Some(_) => true,
            })
        }
        Erasure::Reference(Ref::ActivityTargets) => {
            strings(row, "record.target.kind").contains(&"subject")
                && has(
                    &["record.target.id"],
                    std::slice::from_ref(&context.subject),
                )
        }
        Erasure::Reference(Ref::Documents) => {
            has(
                &[
                    "recipient_subjects",
                    "record.shares.subject",
                    "record.comments.author_subject",
                    "audit.shares.subject",
                    "audit.comments.author_subject",
                ],
                std::slice::from_ref(&context.subject),
            ) || receipt_author(row, &context.subject)
        }
        Erasure::Reference(Ref::Shares) => has(
            &[
                "accepted_subjects",
                "record.link.accepted_subjects",
                "record.link.audience.subjects",
            ],
            std::slice::from_ref(&context.subject),
        ),
        Erasure::Reference(Ref::TriggerRevisions) => {
            has(&["owner"], std::slice::from_ref(&context.subject))
        }
        Erasure::Reference(Ref::Profiles) => {
            has(&["owner"], std::slice::from_ref(&context.subject))
                || [
                    Set::AgentProfiles,
                    Set::WorkspaceProfiles,
                    Set::SandboxProfiles,
                ]
                .iter()
                .any(|set| has(&["_id", "profile_id"], &context.ids(*set)))
        }
        Erasure::Reference(Ref::DiagnosticBindings) => {
            row.get_str("_id")
                .is_ok_and(|id| id.starts_with("diagnostic:"))
                && (has(
                    &["record.owner_subject"],
                    std::slice::from_ref(&context.subject),
                ) || has(&["record.sandbox_id"], &context.ids(Set::Sandboxes)))
        }
        Erasure::Reference(Ref::PlatformSettings) => super::references::settings_mentions(
            row,
            &std::collections::BTreeSet::from([context.subject.clone()]),
        ),
        // These bounded receipt codecs perform their own precise
        // precheck before decoding. They never recursively match arbitrary text.
        Erasure::Reference(Ref::ShareReceipts | Ref::LogGates | Ref::Identities) => true,
        _ => false,
    }
}
fn receipt_author(row: &Document, subject: &str) -> bool {
    ["record.receipts", "audit.receipts"].iter().any(|path| {
        super::context::value(row, path)
            .and_then(Bson::as_document)
            .is_some_and(|receipts| {
                receipts.values().any(|receipt| {
                    receipt.as_document().is_some_and(|receipt| {
                        strings(receipt, "outcome.comment.author_subject").contains(&subject)
                    })
                })
            })
    })
}
pub(super) fn destructive(rule: Erasure) -> bool {
    matches!(
        rule,
        Erasure::Owner(_)
            | Erasure::Linked(..)
            | Erasure::LinkedKey(..)
            | Erasure::OwnerKey(_)
            | Erasure::Reference(Ref::ActivityTargets | Ref::DiagnosticBindings)
    )
}
pub(super) fn agents(contexts: &[Context]) -> std::collections::BTreeSet<String> {
    contexts
        .iter()
        .flat_map(|context| context.ids(Set::Agents))
        .collect()
}

/// In-memory vetoes cover every supported lineage field.
pub(super) const PROTECTION_PATHS: &[&str] = &[
    "_id",
    "sandbox_id",
    "record.sandbox_id",
    "record.requester.sandbox_id",
    "record.resources.sandbox_id",
    "agent_id",
    "record.agent_id",
    "record.scope.agent_id",
    "workspace_id",
    "worktree_id",
    "launch_id",
    "binding_id",
    "record.binding_id",
    "record.workspace_id",
    "record.worktree_id",
    "record.launch_id",
];

fn bson_names(value: &Bson, values: &[String]) -> bool {
    match value {
        Bson::String(text) => values.iter().any(|value| value == text),
        Bson::Array(items) => items.iter().any(|item| bson_names(item, values)),
        Bson::Document(row) => row.values().any(|item| bson_names(item, values)),
        _ => false,
    }
}

fn json_names(value: &serde_json::Value, values: &[String]) -> bool {
    match value {
        serde_json::Value::String(text) => values.iter().any(|value| value == text),
        serde_json::Value::Array(items) => items.iter().any(|item| json_names(item, values)),
        serde_json::Value::Object(map) => map.values().any(|item| json_names(item, values)),
        _ => false,
    }
}

#[cfg(test)]
mod json_residue_tests {
    use super::*;
    use mongodb::bson::doc;

    fn context() -> Context {
        let mut context = Context::default();
        context.subject = "subject-erased".into();
        context
            .sets
            .insert(Set::Sandboxes, ["sbx_erased".to_owned()].into());
        context.shares.insert("shr_erased".into());
        context.execution_digests.insert("d1gest".into());
        context
    }

    // #3296: identities inside a serialized record must block erasure.
    #[test]
    fn serialized_records_naming_the_subject_or_resources_fail_closed() {
        let rule = Erasure::ProvenAbsentJson(&["record"]);
        let context = context();
        let named = doc! {"_id": "a", "record": r#"{"intent":{"actor":"subject-erased"}}"#};
        let resource = doc! {"_id": "b", "record": r#"{"key":[{"sandbox":"sbx_erased"}]}"#};
        let unrelated = doc! {"_id": "c", "record": r#"{"actor":"someone-else","n":3}"#};
        let unreadable = doc! {"_id": "d", "record": "{not json"};
        let wrong_type = doc! {"_id": "e", "record": 7_i32};
        let absent = doc! {"_id": "activation", "share_id": "shr_x"};
        let share = doc! {"_id": "h", "record": r#"{"key":{"share_id":"shr_erased"}}"#};
        let digest = doc! {"_id": "i", "record": r#"{"basis":{"actor":"d1gest"}}"#};
        assert!(matches(rule, &digest, &context));
        let receipt = doc! {"_id": "grant_receipt:o", "share_id": "shr_erased"};
        assert!(matches(rule, &share, &context));
        assert!(matches(
            Erasure::ProvenAbsent(&["share_id"]),
            &receipt,
            &context
        ));
        assert!(!matches(
            Erasure::ProvenAbsent(&["share_id"]),
            &absent,
            &context
        ));
        let document = doc! {"_id": "f", "record": {"owner": "someone-else"}};
        let named_document = doc! {"_id": "g", "record": {"owner": ["subject-erased"]}};
        assert!(!matches(rule, &document, &context));
        assert!(matches(rule, &named_document, &context));
        assert!(matches(rule, &named, &context));
        assert!(matches(rule, &resource, &context));
        assert!(!matches(rule, &unrelated, &context));
        assert!(matches(rule, &unreadable, &context));
        assert!(matches(rule, &wrong_type, &context));
        assert!(!matches(rule, &absent, &context));
    }
}

//! A bounded keyset per exact selector; unchanged rows never widen the scan.
use super::{
    catalog::{Entry, Erasure, ReferenceRule},
    context::Context,
    Error,
};
use crate::store::durability::majority_collection;
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Bson, Document},
    options::Hint,
    Database,
};
use std::collections::BTreeSet;

const PAGE: i64 = 500;
const EMPTY_PROBES: usize = 8;

pub(super) mod logs;

/// Semantic positions survive a preceding selector disappearing from the cache.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub(super) struct Key {
    field: String,
    value: String,
    prefix: bool,
}
impl Key {
    pub(super) fn query(&self, after: Option<&Bson>) -> Document {
        let value = if self.prefix {
            Bson::Document(doc! {"$regex": format!("^{}", super::selectors::regex(&self.value))})
        } else {
            Bson::String(self.value.clone())
        };
        let mut filter = doc! {&self.field: value};
        // Preserve BSON ordering for mixed legacy string/ObjectId identities.
        if let Some(after) = after {
            filter.insert("$expr", doc! {"$gt":["$_id", {"$literal":after}]});
        }
        filter
    }
    pub(super) fn hint(&self) -> Hint {
        Hint::Keys(if self.field == "_id" {
            doc! {"_id":1}
        } else {
            doc! {&self.field:1,"_id":1}
        })
    }
}
pub(super) struct Page {
    pub rows: Vec<Document>,
    pub selector: Option<Key>,
    pub after: Option<Bson>,
    pub complete: bool,
}
pub(super) fn fields(entry: &Entry) -> BTreeSet<&'static str> {
    entry
        .rules
        .iter()
        .filter(|rule| rule.indexed)
        .flat_map(|rule| match rule.erasure {
            Erasure::Owner(paths) | Erasure::Linked(_, paths) => paths.to_vec(),
            Erasure::Reference(ReferenceRule::Shares) => vec!["accepted_subjects"],
            _ => vec!["_id"],
        })
        .collect()
}
fn keys(entry: &Entry, contexts: &[Context]) -> Vec<Key> {
    let mut keys = BTreeSet::new();
    for context in contexts {
        for rule in entry.rules.iter().filter(|rule| rule.indexed) {
            let (fields, values, prefix) = match rule.erasure {
                Erasure::Owner(paths) => (paths.to_vec(), vec![context.subject.clone()], false),
                Erasure::Linked(set, paths) => (paths.to_vec(), context.ids(set), false),
                Erasure::LinkedKey(set, prefix) => (
                    vec!["_id"],
                    context
                        .ids(set)
                        .iter()
                        .map(|id| format!("{prefix}{id}:"))
                        .collect(),
                    true,
                ),
                Erasure::OwnerKey(key) => {
                    (vec!["_id"], super::selectors::keys(context, key), false)
                }
                Erasure::Reference(ReferenceRule::Shares) => (
                    vec!["accepted_subjects"],
                    vec![context.subject.clone()],
                    false,
                ),
                _ => continue,
            };
            for field in fields {
                for value in &values {
                    keys.insert(Key {
                        field: field.into(),
                        value: value.clone(),
                        prefix,
                    });
                }
            }
        }
    }
    keys.into_iter().collect()
}

/// At most eight empty equality ranges and one nonempty page per invocation.
/// Protection is evaluated after reading, so a page examines at most 500 rows.
pub(super) async fn page(
    db: &Database,
    entry: &Entry,
    contexts: &[Context],
    selector: Option<&Key>,
    after: Option<&Bson>,
) -> Result<Page, Error> {
    let page_size = if super::resource_execution::managed(entry.collection) {
        8
    } else {
        PAGE
    };
    let order = logs::available(db, entry.collection).await?;
    let keys = keys(entry, contexts);
    let mut index = selector.map_or(0, |selector| keys.partition_point(|key| key < selector));
    let mut position = after.cloned();
    if selector.is_some_and(|selector| keys.get(index) != Some(selector)) {
        position = None;
    }
    for _ in 0..EMPTY_PROBES {
        let Some(key) = keys.get(index) else {
            return Ok(Page {
                rows: vec![],
                selector: None,
                after: None,
                complete: true,
            });
        };
        let filter = match order {
            Some(order) => order.query(key.query(None), position.as_ref())?,
            None => key.query(position.as_ref()),
        };
        let rows: Vec<Document> = majority_collection(db, entry.collection)
            .find(filter)
            .sort(order.map_or_else(|| doc! {"_id":1}, logs::Order::sort))
            .hint(order.map_or_else(|| key.hint(), logs::Order::hint))
            .limit(page_size)
            .await?
            .try_collect()
            .await?;
        let length = rows.len();
        let (next, after) = if length < page_size as usize {
            index += 1;
            (keys.get(index).cloned(), None)
        } else {
            let row = rows.last().unwrap();
            let after = match order {
                Some(order) => Some(order.position(row, entry.collection)?),
                None => row.get("_id").cloned(),
            };
            (Some(key.clone()), after)
        };
        if !rows.is_empty() {
            return Ok(Page {
                rows,
                complete: next.is_none(),
                selector: next,
                after,
            });
        }
        position = None;
    }
    Ok(Page {
        rows: vec![],
        selector: keys.get(index).cloned(),
        after: None,
        complete: index >= keys.len(),
    })
}

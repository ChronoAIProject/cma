//! Persist one indexed reference page per invocation before any erasure effect.
use super::*;
use futures_util::TryStreamExt;
use mongodb::options::Hint;
use std::collections::BTreeMap;
#[path = "graph.rs"]
mod graph;
#[path = "sources.rs"]
mod sources;
const PAGE: i64 = 500;
const MAX_PROFILE_IDS: usize = 20_000;
const MAX_PROFILE_LINKS: usize = 100_000;

#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(in crate::store::account_erasure) struct State {
    source: usize,
    owner_index: usize,
    after: Option<Bson>,
    // Tuple values keep the bounded ownership map compact in JSON and BSON.
    owners: BTreeMap<String, (String, u8)>,
    edges: BTreeMap<String, BTreeSet<String>>,
    roots: BTreeSet<String>,
    owned_counts: BTreeMap<String, usize>,
    links: BTreeMap<String, usize>,
}
pub(super) struct Retained {
    pub pins: BTreeSet<String>,
    pub owners: BTreeMap<String, String>,
}
impl State {
    fn collect_roots(&mut self, value: Option<&Bson>) -> Result<(), Error> {
        let mut found = BTreeSet::new();
        graph::collect(value, &mut found);
        for pin in found {
            let Some((id, _)) = pin.rsplit_once(':') else {
                continue;
            };
            if self.owners.contains_key(id) {
                self.roots.insert(pin);
            }
        }
        Ok(())
    }
    fn finish(&mut self) -> Result<Retained, Error> {
        let mut pending: Vec<_> = self.roots.iter().cloned().collect();
        while let Some(pin) = pending.pop() {
            let (id, _) = pin
                .rsplit_once(':')
                .ok_or_else(|| Error::Store("invalid retained pin".into()))?;
            let (subject, kind) = self
                .owners
                .get(id)
                .ok_or_else(|| Error::Store("retained owner is missing".into()))?;
            let children = self
                .edges
                .get(&pin)
                .ok_or_else(|| {
                    Error::InvalidRecord(REVISIONS[usize::from(*kind)]).attribute(subject)
                })?
                .clone();
            for child in children {
                let Some((id, _)) = child.rsplit_once(':') else {
                    continue;
                };
                if self.owners.contains_key(id) && self.roots.insert(child.clone()) {
                    pending.push(child);
                }
            }
        }
        let owners: BTreeMap<_, _> = self
            .roots
            .iter()
            .filter_map(|pin| {
                let (id, _) = pin.rsplit_once(':')?;
                Some((id.to_owned(), self.owners.get(id)?.0.clone()))
            })
            .collect();
        self.roots.extend(owners.keys().cloned());
        Ok(Retained {
            pins: std::mem::take(&mut self.roots),
            owners,
        })
    }
    pub(in crate::store::account_erasure) fn contribution(
        &self,
        subject: &str,
    ) -> Result<usize, Error> {
        #[derive(serde::Serialize)]
        struct Portion<'a> {
            owners: BTreeMap<&'a String, &'a (String, u8)>,
            edges: BTreeMap<&'a String, &'a BTreeSet<String>>,
            roots: Vec<&'a String>,
        }
        let belongs = |pin: &str| {
            self.owners
                .get(pin.rsplit_once(':').map_or(pin, |(id, _)| id))
                .is_some_and(|(owner, _)| owner == subject)
        };
        super::super::progress::serialized_bytes(&Portion {
            owners: self
                .owners
                .iter()
                .filter(|(_, (owner, _))| owner == subject)
                .collect(),
            edges: self.edges.iter().filter(|(pin, _)| belongs(pin)).collect(),
            roots: self.roots.iter().filter(|pin| belongs(pin)).collect(),
        })
    }
}

pub(super) async fn page(
    db: &Database,
    subjects: &BTreeSet<String>,
    organizations: &BTreeSet<String>,
    state: &mut State,
) -> Result<Option<Retained>, Error> {
    let sources = sources::sources();
    if state.source > 9 + sources.len() {
        return Err(Error::Store("invalid reference discovery source".into()));
    }
    if state.source == 9 + sources.len() || (state.source >= 3 && state.owners.is_empty()) {
        return state.finish().map(Some);
    }
    let own = state.source < 3;
    let (name, owner_path, mut projection) = if own {
        (AGGREGATES[state.source], Some("owner"), doc! {"owner":1})
    } else if state.source < 6 {
        (
            AGGREGATES[state.source - 3],
            Some("owner"),
            doc! {"owner":1,"record.draft.workspace_profiles":1,"record.draft.agent_profiles":1,"record.forked_from":1},
        )
    } else if state.source < 9 {
        (
            REVISIONS[state.source - 6],
            None,
            doc! {"document_schema_version":1,"profile_id":1,"revision":1,
            "record.profile_id":1,"record.revision":1,"record.content.workspace_profiles":1,"record.content.agent_profiles":1},
        )
    } else {
        let (name, owner, fields) = &sources[state.source - 9];
        let mut projection = doc! {};
        for field in fields {
            projection.insert(*field, 1);
        }
        if let Some(owner) = owner {
            projection.insert(*owner, 1);
        }
        if matches!(*name, "sandboxes" | "workspaces" | "agents") {
            projection.insert("sandbox_id", 1);
        }
        (*name, *owner, projection)
    };
    let mut filter = doc! {};
    if own {
        let subject = subjects
            .iter()
            .nth(state.owner_index)
            .ok_or_else(|| Error::Store("invalid discovery member".into()))?;
        filter.insert("owner", subject);
    }
    if let Some(after) = &state.after {
        filter.insert("$expr", doc! {"$gt":["$_id",{"$literal":after}]});
    }
    // _id remains implicitly projected. Every page follows the same BSON sort
    // order, including a legacy ObjectId after the last string identity.
    projection.remove("_id");
    let hint = if own {
        Hint::Keys(doc! {"owner":1,"_id":1})
    } else {
        Hint::Name("_id_".into())
    };
    let rows: Vec<Document> = majority_collection::<Document>(db, name)
        .find(filter)
        .projection(projection)
        .sort(doc! {"_id":1})
        .hint(hint)
        .limit(PAGE)
        .await?
        .try_collect()
        .await?;
    for row in &rows {
        if own {
            let subject = subjects.iter().nth(state.owner_index).unwrap();
            let id = row
                .get_str("_id")
                .map_err(|_| Error::InvalidRecord(name).attribute(subject))?;
            let count = state.owned_counts.entry(subject.clone()).or_default();
            *count += 1;
            if *count > MAX_PROFILE_IDS {
                return Err(Error::capacity(subject, "owned Profiles", MAX_PROFILE_IDS));
            }
            state
                .owners
                .insert(id.into(), (subject.clone(), state.source as u8));
        } else if state.source < 9 && state.source >= 6 {
            let owned = graph::owned_revision(name, row, &state.owners)?
                .map(|(id, owner, rev)| (id.to_owned(), owner.to_owned(), rev));
            if let Some((id, subject, revision)) = owned {
                let mut children = BTreeSet::new();
                graph::collect(row.get("record"), &mut children);
                let count = state.links.entry(subject.clone()).or_default();
                *count += children.len() + 1;
                if *count > MAX_PROFILE_LINKS {
                    return Err(Error::capacity(
                        &subject,
                        "Profile revision links",
                        MAX_PROFILE_LINKS,
                    ));
                }
                state.edges.insert(graph::key(&id, revision), children);
            } else {
                state.collect_roots(row.get("record"))?;
            }
        } else {
            // Valid organization policy overrides personal attribution even if
            // a legacy subject namespace overlaps. Descendants share that root.
            let organization = row
                .get_str("_id")
                .is_ok_and(|id| organizations.contains(id))
                || row
                    .get_str("sandbox_id")
                    .is_ok_and(|id| organizations.contains(id));
            let deleting = owner_path
                .and_then(|path| super::super::context::value(row, path))
                .and_then(Bson::as_str)
                .is_some_and(|owner| subjects.contains(owner));
            if !deleting || organization {
                state.collect_roots(Some(&Bson::Document(row.clone())))?;
            }
        }
    }
    state.after = rows.last().and_then(|row| row.get("_id")).cloned();
    if rows.len() < PAGE as usize {
        state.after = None;
        if own && state.owner_index + 1 < subjects.len() {
            state.owner_index += 1;
        } else {
            state.owner_index = 0;
            state.source += 1;
        }
    }
    if state.source == 9 + sources.len() || (state.source >= 3 && state.owners.is_empty()) {
        state.finish().map(Some)
    } else {
        Ok(None)
    }
}

//! Reviewed query definitions; adding a query never changes the operation engine.
use super::super::{
    activity_audit_index, activity_page_index, agent_owner_index, steps::Step, Result,
};
use mongodb::bson::{doc, Document};
use serde_json::{json, Value};

#[derive(Clone, Copy)]
pub(super) enum Binding {
    Static,
    Equality(&'static str),
    UsageInterval,
}

#[derive(Clone)]
pub(super) struct Query {
    pub name: &'static str,
    pub collection: &'static str,
    pub filter: Document,
    pub binding: Binding,
    pub qualification: &'static str,
    pub sort: Document,
    pub projection: Document,
    pub limit: i64,
    pub index: Document,
    pub step: Option<Step>,
}
impl Query {
    pub fn migration(&self) -> Result<Step> {
        self.step
            .ok_or("query is inspection-only; no build, status or abort manifest")
    }
    pub fn index_name(&self) -> &str {
        self.index.get_str("name").expect("reviewed index name")
    }
    pub fn safe_fields(&self) -> Vec<&str> {
        self.index
            .get_document("key")
            .expect("reviewed index keys")
            .keys()
            .chain(self.sort.keys())
            .map(String::as_str)
            .collect()
    }
    pub fn describe(&self) -> Value {
        let mut shape = self.filter.clone();
        if let Binding::Equality(field) = self.binding {
            shape.insert(field, doc! {"$eq":"<sampled string>"});
        }
        if matches!(self.binding, Binding::UsageInterval) {
            shape = doc! {"sandbox_id":{"$in":["<sampled string>"]},
            "ended_at":{"$gt":"<sampled UTC day start: integer seconds>"},
            "started_at":{"$lt":"<sampled UTC day end: integer seconds>"}};
        }
        json!({"name":self.name,"collection":self.collection,"filter_shape":shape,
            "sort":self.sort,"projection":self.projection,"limit":self.limit,
            "index":self.index,"manifest":self.step.map(|s|s.id()),"checksum":self.step.map(|s|s.checksum()),
            "inspection_only":self.step.is_none(),"qualification":self.qualification})
    }
}

pub(super) fn activity() -> Query {
    Query {
        name: "activity-first-page",
        collection: "activity",
        filter: doc! {},
        binding: Binding::Static,
        qualification: "exact unfiltered first-page query",
        sort: doc! {"recorded_at":-1,"_id":-1},
        projection: doc! {"_id":1,"document_schema_version":1,"recorded_at":1,
        "record.id":1,"record.at":1,"record.actor":1,"record.action":1,
        "record.target":1,"record.outcome":1},
        limit: 51,
        index: activity_page_index::indexes()[0].clone(),
        step: Some(Step::ActivityPage),
    }
}

pub(super) fn agents_owner() -> Query {
    Query {
        name: "agents-owner-page",
        collection: "agents",
        filter: doc! {},
        binding: Binding::Equality("owner"),
        qualification: "one sampled owner; not all owner strata",
        sort: doc! {"_id":1},
        projection: doc! {},
        limit: 51,
        index: agent_owner_index::index(),
        step: Some(Step::AgentOwnerPage),
    }
}

pub(super) fn all() -> Vec<Query> {
    let mut audit = activity();
    audit.name = "activity-audit-first-page";
    audit.filter = activity_audit_index::filter();
    audit.qualification = "audit-first predicate over retained Activity records";
    audit.index = activity_audit_index::index();
    audit.step = Some(Step::ActivityAudit);
    let mut queries = vec![activity(), audit];
    queries.extend(super::candidates::usage());
    queries.push(agents_owner());
    queries.extend(super::candidates::claims());
    queries
}

pub(super) fn select(names: &str) -> Result<Vec<Query>> {
    let catalog = all();
    let mut selected = Vec::new();
    for name in names.split(',') {
        if selected.len() == 8 || selected.iter().any(|q: &Query| q.name == name) {
            return Err("index query selection exceeds bound or contains duplicates");
        }
        let query = catalog
            .iter()
            .find(|q| q.name == name)
            .ok_or("unknown index query name")?;
        selected.push(query.clone());
    }
    Ok(selected)
}

pub(super) fn one(name: &str) -> Result<Query> {
    let mut queries = select(name)?;
    if queries.len() != 1 {
        return Err("index mutation requires exactly one named query");
    }
    let query = queries.remove(0);
    query.migration()?;
    Ok(query)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_is_explicit_and_each_index_has_a_complete_manifest() {
        assert_eq!(select("activity-first-page").unwrap().len(), 1);
        for invalid in [
            "",
            "activity",
            "activity-first-page,activity-first-page",
            "activity-first-page,unknown",
        ] {
            assert!(select(invalid).is_err());
        }
        for q in all() {
            if let Some(step) = q.step {
                assert_eq!(step.collections(), &[q.collection]);
                assert_eq!(step.max_changes(), 1);
            } else {
                assert!(one(q.name).is_err());
                assert!(q.describe()["inspection_only"].as_bool().unwrap());
            }
            assert!((1..=1000).contains(&q.limit));
            assert!((2..=3).contains(&q.index.len()));
        }
    }
}

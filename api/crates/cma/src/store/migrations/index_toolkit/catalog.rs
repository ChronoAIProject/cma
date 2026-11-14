//! Reviewed query definitions; adding a query never changes the operation engine.
use super::super::{activity_page_index, agent_owner_index, steps::Step, Result};
use mongodb::bson::{doc, Document};
use serde_json::{json, Value};

#[derive(Clone)]
pub(super) struct Query {
    pub name: &'static str,
    pub collection: &'static str,
    pub filter: Document,
    pub sample_equality: Option<&'static str>,
    pub sort: Document,
    pub projection: Document,
    pub limit: i64,
    pub index: Document,
    pub step: Step,
}
impl Query {
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
        if let Some(field) = self.sample_equality {
            shape.insert(field, doc! {"$eq":"<sampled string>"});
        }
        json!({"name":self.name,"collection":self.collection,"filter_shape":shape,
            "sort":self.sort,"projection":self.projection,"limit":self.limit,
            "index":self.index,"manifest":self.step.id(),"checksum":self.step.checksum()})
    }
}

pub(super) fn activity() -> Query {
    Query {
        name: "activity-first-page",
        collection: "activity",
        filter: doc! {},
        sample_equality: None,
        sort: doc! {"recorded_at":-1,"_id":-1},
        projection: doc! {"_id":1,"document_schema_version":1,"recorded_at":1,
        "record.id":1,"record.at":1,"record.actor":1,"record.action":1,
        "record.target":1,"record.outcome":1},
        limit: 51,
        index: activity_page_index::indexes()[0].clone(),
        step: Step::ActivityPage,
    }
}

pub(super) fn agents_owner() -> Query {
    Query {
        name: "agents-owner-page",
        collection: "agents",
        filter: doc! {},
        sample_equality: Some("owner"),
        sort: doc! {"_id":1},
        projection: doc! {},
        limit: 51,
        index: agent_owner_index::index(),
        step: Step::AgentOwnerPage,
    }
}

pub(super) fn all() -> Vec<Query> {
    vec![activity(), agents_owner()]
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
    Ok(queries.remove(0))
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
            assert_eq!(q.step.collections(), &[q.collection]);
            assert_eq!(q.step.max_changes(), 1);
            assert!((1..=1000).contains(&q.limit));
            assert_eq!(q.index.len(), 2);
        }
    }
}

//! Reuse frozen carrier indexes without preparing or changing their DDL.
use super::*;
use mongodb::{error::ErrorKind, IndexModel};

#[derive(Clone, Copy)]
pub(super) struct Order(&'static [&'static str]);

fn specification(name: &str) -> Option<Order> {
    Some(Order(match name {
        "agent_log_segments" => &["incarnation", "ordinal"],
        "agent_log_publications" | "agent_log_admissions" => &["incarnation", "publication_key"],
        "agent_log_evidence" => &["incarnation", "evidence_generation", "_id"],
        "agent_log_audits" => &["audit_id"],
        "agent_capacity_recovery_work" => &["format_version", "_id"],
        _ => return None,
    }))
}

pub(in crate::store::account_erasure) fn managed(name: &str) -> bool {
    specification(name).is_some()
}

pub(in crate::store::account_erasure) fn installed(name: &str, indexes: &[IndexModel]) -> bool {
    specification(name).is_some_and(|order| indexes.iter().any(|index| order.matches(index)))
}

pub(super) async fn available(db: &Database, name: &str) -> Result<Option<Order>, Error> {
    let Some(order) = specification(name) else {
        return Ok(None);
    };
    let indexes = match db.collection::<Document>(name).list_indexes().await {
        Ok(cursor) => cursor.try_collect::<Vec<_>>().await?,
        Err(error) if matches!(error.kind.as_ref(), ErrorKind::Command(command) if command.code == 26) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    Ok(indexes
        .iter()
        .any(|index| order.matches(index))
        .then_some(order))
}

impl Order {
    fn keys(self) -> Document {
        std::iter::once(("agent_id".into(), Bson::Int32(1)))
            .chain(self.0.iter().map(|key| ((*key).into(), Bson::Int32(1))))
            .collect()
    }

    fn matches(self, index: &IndexModel) -> bool {
        index.keys.iter().eq(self.keys().iter())
            && index.options.as_ref().is_some_and(|options| {
                options.sparse != Some(true)
                    && options.hidden != Some(true)
                    && options.partial_filter_expression.is_none()
                    && (self.0.contains(&"_id") || options.unique == Some(true))
            })
    }

    pub(super) fn hint(self) -> Hint {
        Hint::Keys(self.keys())
    }

    pub(super) fn sort(self) -> Document {
        self.0
            .iter()
            .map(|key| ((*key).into(), Bson::Int32(1)))
            .collect()
    }

    pub(super) fn query(
        self,
        mut filter: Document,
        after: Option<&Bson>,
    ) -> Result<Document, Error> {
        if let Some(after) = after {
            let position = after
                .as_document()
                .ok_or(Error::InvalidRecord("log keyset cursor"))?;
            let mut equal = Document::new();
            let mut ranges = Vec::new();
            for key in self.0 {
                let value = position
                    .get(*key)
                    .ok_or(Error::InvalidRecord("log keyset cursor"))?;
                let mut range = equal.clone();
                range.insert(*key, doc! {"$gt":value});
                ranges.push(range);
                equal.insert(*key, value.clone());
            }
            filter.insert("$or", ranges);
        }
        Ok(filter)
    }

    pub(super) fn position(self, row: &Document, name: &'static str) -> Result<Bson, Error> {
        self.0
            .iter()
            .map(|key| {
                row.get(*key)
                    .cloned()
                    .map(|value| ((*key).into(), value))
                    .ok_or(Error::InvalidRecord(name))
            })
            .collect::<Result<Document, Error>>()
            .map(Bson::Document)
    }
}

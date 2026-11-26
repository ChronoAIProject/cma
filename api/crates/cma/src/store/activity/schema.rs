//! Read-only activity metadata inspection; retention is never inferred from absence.

use std::time::Duration;

use mongodb::{
    bson::{doc, Document},
    Database,
};

use crate::store::{
    migrations,
    schema::{inspect_collection, Assessment, SchemaFinding},
};

pub(super) async fn inspect(
    database: &Database,
    retention: Duration,
) -> Result<Assessment<bool>, SchemaFinding> {
    let mut assessment = Assessment::default();
    let Some(existing) = inspect_collection(database, super::ACTIVITY).await? else {
        assessment.findings.insert(SchemaFinding::MissingCollection);
        return Ok(assessment);
    };
    match migrations::missing_activity_indexes(&existing) {
        Ok(pending) if pending.is_empty() => {}
        Ok(_) => {
            assessment.findings.insert(SchemaFinding::MissingQueryIndex);
        }
        Err(_) => {
            assessment
                .findings
                .insert(SchemaFinding::DefinitionMismatch);
        }
    }
    if let Some(finding) = ttl_finding(&existing, retention) {
        assessment.findings.insert(finding);
    }
    match migrations::activity_audit_index::ready(&existing) {
        Ok(ready) => assessment.capability = ready,
        Err(_) => {
            assessment
                .findings
                .insert(SchemaFinding::DefinitionMismatch);
        }
    }
    Ok(assessment)
}

fn ttl_finding(existing: &[Document], retention: Duration) -> Option<SchemaFinding> {
    let Ok(seconds) = i64::try_from(retention.as_secs()) else {
        return Some(SchemaFinding::DefinitionMismatch);
    };
    if retention.subsec_nanos() != 0 {
        return Some(SchemaFinding::DefinitionMismatch);
    }
    let desired =
        doc! {"name":"activity_expiry", "key":{"recorded_at":1}, "expireAfterSeconds":seconds};
    let mut matches = existing.iter().filter(|index| {
        index.get_str("name").ok() == Some("activity_expiry")
            || index.get_document("key").ok() == Some(&doc! {"recorded_at":1})
            || index.contains_key("expireAfterSeconds")
    });
    let Some(index) = matches.next() else {
        return Some(SchemaFinding::TtlAbsent);
    };
    if matches.next().is_some() {
        return Some(SchemaFinding::DefinitionMismatch);
    }
    let mut index = index.clone();
    if let Ok(seconds) = index.get_i32("expireAfterSeconds") {
        index.insert("expireAfterSeconds", i64::from(seconds));
    }
    match crate::store::schema::index_contract_v1::missing(&[index], &desired) {
        Ok(pending) if pending.is_empty() => None,
        _ => Some(SchemaFinding::DefinitionMismatch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ttl_absence_and_any_policy_or_option_change_are_distinct_findings() {
        let retention = Duration::from_secs(86400);
        assert_eq!(ttl_finding(&[], retention), Some(SchemaFinding::TtlAbsent));
        let index =
            doc! {"name":"activity_expiry","key":{"recorded_at":1},"expireAfterSeconds":86400_i32};
        assert_eq!(ttl_finding(std::slice::from_ref(&index), retention), None);
        for (key, value) in [
            ("expireAfterSeconds", mongodb::bson::Bson::Int64(43200)),
            ("expireAfterSeconds", 172800_i64.into()),
            ("expireAfterSeconds", 86400.0_f64.into()),
            ("hidden", true.into()),
            ("partialFilterExpression", doc! {"reviewed":false}.into()),
            ("collation", doc! {"locale":"en"}.into()),
            ("key", doc! {"wrong_timestamp":1}.into()),
        ] {
            let mut drift = index.clone();
            drift.insert(key, value);
            assert_eq!(
                ttl_finding(&[drift], retention),
                Some(SchemaFinding::DefinitionMismatch)
            );
        }
        assert_eq!(
            ttl_finding(&[index.clone(), index], retention),
            Some(SchemaFinding::DefinitionMismatch)
        );
    }
}

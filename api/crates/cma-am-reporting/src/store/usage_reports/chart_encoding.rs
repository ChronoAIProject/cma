//! Preserve the stored chart schema while encoding each payload only once.
use super::*;
use layer2_core::sandbox_metrics::{allocation::Expectation, series::Bucket};
use mongodb::bson::{DateTime, RawDocumentBuf};

#[derive(Serialize)]
pub(super) struct ChartRecord<'a> {
    pub buckets: &'a [Bucket],
    pub expectations: &'a [Expectation],
}

#[derive(Serialize)]
pub(super) struct ChartDocument<'a> {
    #[serde(rename = "_id")]
    pub id: String,
    pub document_schema_version: i64,
    pub report_id: &'a str,
    pub sandbox_id: &'a str,
    pub owner: &'a str,
    pub group: &'a str,
    pub series_id: &'a str,
    pub metric: i64,
    pub chunk: i64,
    pub rank: f64,
    pub expires_at: DateTime,
    pub record: ChartRecord<'a>,
}

impl ChartDocument<'_> {
    pub(super) fn encode(&self) -> Result<RawDocumentBuf> {
        let bytes = mongodb::bson::serialize_to_vec(self).map_err(unavailable)?;
        RawDocumentBuf::from_bytes(bytes).map_err(unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_chart_matches_existing_bson_schema_for_missing_and_present_points() {
        for buckets in [Vec::new(), vec![Bucket::default()]] {
            let expectations = vec![Expectation {
                from: 123,
                to: 456,
                running: Some(false),
                home: None,
            }];
            let chart = ChartDocument {
                id: "report:sandbox:0:0".into(),
                document_schema_version: 1,
                report_id: "report",
                sandbox_id: "sandbox",
                owner: "fixture",
                group: "sandbox",
                series_id: "sandbox",
                metric: 0,
                chunk: 0,
                rank: 0.0,
                expires_at: expires(1000),
                record: ChartRecord {
                    buckets: &buckets,
                    expectations: &expectations,
                },
            };
            let expected = doc! {
                "_id": "report:sandbox:0:0", "document_schema_version":1_i64,
                "report_id":"report", "sandbox_id":"sandbox", "owner":"fixture",
                "group":"sandbox", "series_id":"sandbox", "metric":0_i64,
                "chunk":0_i64,"rank":0.0,"expires_at":expires(1000),
                "record": {"buckets":encode(&buckets).unwrap(),"expectations":encode(&expectations).unwrap()},
            };
            let actual = Document::try_from(chart.encode().unwrap()).unwrap();
            assert_eq!(actual, expected);
        }
    }
}

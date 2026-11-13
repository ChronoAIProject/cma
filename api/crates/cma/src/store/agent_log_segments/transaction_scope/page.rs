//! Fixed row and byte budgets for caller-owned transaction enumeration.
use futures_util::{Stream, TryStreamExt};
use mongodb::{bson::Document, error::Error};

const MAX_ROWS: u32 = 256;
const MAX_BYTES: usize = 4 * 1024 * 1024;

pub(super) fn validate_limit(limit: u32) -> Result<(), Error> {
    if !(1..=MAX_ROWS).contains(&limit) {
        return Err(Error::custom("transaction page limit must be 1..=256"));
    }
    Ok(())
}

pub(super) async fn collect(
    source: impl Stream<Item = Result<Document, Error>>,
    limit: u32,
) -> Result<Vec<Document>, Error> {
    validate_limit(limit)?;
    futures_util::pin_mut!(source);
    let mut rows = Vec::with_capacity(limit as usize);
    let mut bytes = 0_usize;
    while let Some(row) = source.try_next().await? {
        let size = mongodb::bson::serialize_to_vec(&row)
            .map_err(|_| Error::custom("transaction page BSON is invalid"))?
            .len();
        bytes = bytes
            .checked_add(size)
            .filter(|bytes| *bytes <= MAX_BYTES)
            .ok_or_else(|| Error::custom("transaction page exceeds its byte budget"))?;
        if rows.len() >= limit as usize {
            return Err(Error::custom("transaction page exceeds its row budget"));
        }
        rows.push(row);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;
    use mongodb::bson::doc;

    #[tokio::test]
    async fn transaction_page_preserves_order_and_refuses_partial_driver_results() {
        let rows = vec![doc! {"_id":"a"}, doc! {"_id":"b"}];
        assert_eq!(
            collect(stream::iter(rows.clone().into_iter().map(Ok)), 2)
                .await
                .unwrap(),
            rows
        );
        assert!(collect(
            stream::iter([Ok(rows[0].clone()), Err(Error::custom("read unavailable"))]),
            2,
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn transaction_page_rejects_zero_unbounded_and_excess_rows() {
        for limit in [0, 257, u32::MAX] {
            assert!(collect(stream::empty(), limit).await.is_err());
        }
        assert!(collect(stream::iter([Ok(doc! {}), Ok(doc! {})]), 1)
            .await
            .is_err());
        assert!(
            collect(stream::iter((0..256).map(|n| Ok(doc! {"n":n}))), 256)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn transaction_page_enforces_combined_bson_bytes_without_truncation() {
        // BSON includes its envelope; each row individually fits the budget.
        let row = doc! {"value":"x".repeat(MAX_BYTES / 2)};
        assert!(collect(stream::iter([Ok(row.clone())]), 1).await.is_ok());
        assert!(collect(stream::iter([Ok(row.clone()), Ok(row)]), 2)
            .await
            .is_err());
        assert!(
            collect(stream::iter([Ok(doc! {"value":"x".repeat(MAX_BYTES)})]), 1)
                .await
                .is_err()
        );
    }
}

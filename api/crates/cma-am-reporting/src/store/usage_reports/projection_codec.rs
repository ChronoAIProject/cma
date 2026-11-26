use super::*;
use flate2::{write::ZlibEncoder, Compression, Decompress, FlushDecompress, Status};
use layer2_core::sandbox_metrics::Point;
use mongodb::bson::{spec::BinarySubtype, Binary};
use serde::Deserialize;
use std::io::Write;

const CODEC_VERSION: i64 = 1;
const MAX_DECODED_BYTES: usize = 128 * 1024;
const MAX_STORED_BYTES: usize = MAX_DECODED_BYTES + 1024;

#[derive(Serialize)]
struct PointPayload<'a> {
    points: &'a [Point],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecodedPoints {
    points: Vec<Point>,
}

pub(super) fn encode_points(points: &[Point]) -> Result<Document> {
    if points.len() > super::chart_projections::POINTS_PER_CHUNK {
        return Err(MetricsError::Unavailable);
    }
    let raw = mongodb::bson::serialize_to_vec(&PointPayload { points }).map_err(unavailable)?;
    if raw.len() > MAX_DECODED_BYTES {
        return Err(MetricsError::Busy);
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&raw).map_err(unavailable)?;
    let compressed = encoder.finish().map_err(unavailable)?;
    if compressed.len() > MAX_STORED_BYTES {
        return Err(MetricsError::Busy);
    }
    Ok(doc! {
        "point_codec": CODEC_VERSION,
        "decoded_bytes": raw.len() as i64,
        "point_payload": Binary { subtype: BinarySubtype::Generic, bytes: compressed },
    })
}

pub(super) fn decode_points(document: &Document) -> Result<Vec<Point>> {
    if document.get_i64("point_codec").ok() != Some(CODEC_VERSION) {
        return Err(MetricsError::Unavailable);
    }
    let declared = document.get_i64("decoded_bytes").map_err(unavailable)?;
    let declared = usize::try_from(declared).map_err(unavailable)?;
    if !(5..=MAX_DECODED_BYTES).contains(&declared) {
        return Err(MetricsError::Unavailable);
    }
    let compressed = document
        .get_binary_generic("point_payload")
        .map_err(unavailable)?;
    if compressed.len() > MAX_STORED_BYTES {
        return Err(MetricsError::Unavailable);
    }
    let mut decoder = Decompress::new(true);
    // One additional byte distinguishes a full output buffer from a stream
    // whose decoded size exceeds its declared size.
    let mut raw = vec![0; declared + 1];
    let status = decoder
        .decompress(compressed, &mut raw, FlushDecompress::Finish)
        .map_err(unavailable)?;
    if status != Status::StreamEnd
        || decoder.total_in() != compressed.len() as u64
        || decoder.total_out() != declared as u64
    {
        return Err(MetricsError::Unavailable);
    }
    raw.truncate(declared);
    let bson_size = i32::from_le_bytes(raw[..4].try_into().map_err(unavailable)?);
    if usize::try_from(bson_size).ok() != Some(declared) {
        return Err(MetricsError::Unavailable);
    }
    let decoded: DecodedPoints =
        mongodb::bson::deserialize_from_slice(&raw).map_err(unavailable)?;
    if decoded.points.len() > super::chart_projections::POINTS_PER_CHUNK {
        return Err(MetricsError::Unavailable);
    }
    Ok(decoded.points)
}

#[cfg(test)]
mod tests {
    use super::*;
    use layer2_core::sandbox_metrics::series::Bucket;

    #[test]
    fn compressed_projection_page_preserves_order_nulls_and_float_bits() {
        let mut p = Bucket {
            from: 60,
            to: 120,
            mean: Some(-0.0),
            min: Some(1e-150),
            max: Some(1e150),
            covered_seconds: 60.,
            expected_seconds: 60.,
            capacity_integral: 3.0,
            capacity_usage_integral: 1.0 / 3.0,
            ..Bucket::default()
        }
        .point();
        p.unknown_mapping_seconds = f64::from_bits(1);
        let points = vec![p, Bucket::default().point()];
        let original = mongodb::bson::serialize_to_vec(&PointPayload { points: &points }).unwrap();
        let encoded = encode_points(&points).unwrap();
        let decoded = decode_points(&encoded).unwrap();
        assert_eq!(
            original,
            mongodb::bson::serialize_to_vec(&PointPayload { points: &decoded }).unwrap()
        );
        assert_eq!(encode_points(&points).unwrap(), encoded);
    }

    #[test]
    fn compressed_projection_page_refuses_inconsistent_streams() {
        let page = encode_points(&[Bucket::default().point()]).unwrap();
        let compressed = page.get_binary_generic("point_payload").unwrap().clone();
        let malformed_payload = |bytes: Vec<u8>| {
            let mut changed = page.clone();
            changed.insert(
                "point_payload",
                Binary {
                    subtype: BinarySubtype::Generic,
                    bytes,
                },
            );
            changed
        };
        for bytes in [
            compressed[..compressed.len() - 1].to_vec(),
            [compressed.as_slice(), &[0]].concat(),
            [compressed.as_slice(), compressed.as_slice()].concat(),
        ] {
            assert!(decode_points(&malformed_payload(bytes)).is_err());
        }
        let bytes = page.get_i64("decoded_bytes").unwrap();
        for declared in [bytes - 1, bytes + 1, MAX_DECODED_BYTES as i64 + 1] {
            let mut changed = page.clone();
            changed.insert("decoded_bytes", declared);
            assert!(decode_points(&changed).is_err());
        }
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&vec![0; MAX_DECODED_BYTES + 1]).unwrap();
        let mut over_limit = malformed_payload(encoder.finish().unwrap());
        over_limit.insert("decoded_bytes", MAX_DECODED_BYTES as i64);
        assert!(decode_points(&over_limit).is_err());

        let mut changed = page.clone();
        changed.insert("point_codec", CODEC_VERSION + 1);
        assert!(decode_points(&changed).is_err());
    }
}

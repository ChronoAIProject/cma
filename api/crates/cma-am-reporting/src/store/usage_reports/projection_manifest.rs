//! Commit the complete immutable projection lines and allocation order in the
//! generation's parent document. The index is bounded independently of pages.
use super::*;
use mongodb::bson::{spec::BinarySubtype, Binary, Bson, RawDocumentBuf};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
const VERSION: i64 = 3;
const COMMITMENT_BYTES: usize = 40;
const FAMILIES: [&str; 6] = [
    "chart_total",
    "chart_sandbox",
    "chart_owner",
    "chart_explicit_owner",
    "owner_allocation",
    "contributor",
];

// Mongo may move _id to the beginning of a document on insertion. Hash the
// canonical top-level field order, retaining every field and BSON value type.
pub(super) fn document_digest(document: &Document) -> Result<[u8; 32]> {
    let ordered: BTreeMap<_, _> = document.iter().collect();
    let bytes = mongodb::bson::serialize_to_vec(&ordered).map_err(unavailable)?;
    Ok(Sha256::digest(bytes).into())
}

fn line_key(kind: &str, metric: i64, series: &str) -> Result<String> {
    let key = doc! {"kind":kind,"metric":metric,"series":series};
    let bytes = mongodb::bson::serialize_to_vec(&key).map_err(unavailable)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[derive(Default)]
struct LineChunks {
    chunks: BTreeMap<i64, [u8; 32]>,
}
impl LineChunks {
    fn commitment(self) -> Result<Binary> {
        let mut hash = Sha256::new();
        let count = self.chunks.len() as u64;
        for (expected, (chunk, digest)) in self.chunks.into_iter().enumerate() {
            if chunk != expected as i64 {
                return Err(MetricsError::Unavailable);
            }
            hash.update(digest);
        }
        let mut bytes = Vec::with_capacity(COMMITMENT_BYTES);
        bytes.extend(count.to_be_bytes());
        bytes.extend(hash.finalize());
        Ok(Binary {
            subtype: BinarySubtype::Generic,
            bytes,
        })
    }
}

pub struct ProjectionManifest {
    pub generation: i64,
    families: BTreeSet<String>,
    lines: Document,
    allocations: Document,
}
impl ProjectionManifest {
    pub fn build(
        generation: i64,
        documents: &[RawDocumentBuf],
        page_bytes: i64,
    ) -> Result<Document> {
        let actual_page_bytes = documents.iter().try_fold(0_i64, |bytes, document| {
            bytes
                .checked_add(document.as_bytes().len() as i64)
                .ok_or(MetricsError::Busy)
        })?;
        if page_bytes != actual_page_bytes {
            return Err(MetricsError::Unavailable);
        }
        let mut digest = Sha256::new();
        let mut families = BTreeMap::<String, (i64, Sha256)>::new();
        let mut lines = BTreeMap::<String, LineChunks>::new();
        let mut allocations = BTreeMap::<i64, BTreeMap<i64, Document>>::new();
        for raw in documents {
            digest.update(raw.as_bytes());
            let kind = raw.get_str("kind").map_err(unavailable)?;
            if !FAMILIES.contains(&kind) || raw.get_i64("generation").ok() != Some(generation) {
                return Err(MetricsError::Unavailable);
            }
            let family = families.entry(kind.to_owned()).or_default();
            family.0 += 1;
            family.1.update(raw.as_bytes());
            if kind == "contributor" {
                continue;
            }
            let document = Document::try_from(raw.clone()).map_err(unavailable)?;
            let metric = document.get_i64("metric").map_err(unavailable)?;
            let series = document.get_str("series").map_err(unavailable)?;
            let chunk = document.get_i64("chunk").map_err(unavailable)?;
            let key = line_key(kind, metric, series)?;
            if lines
                .entry(key)
                .or_default()
                .chunks
                .insert(chunk, document_digest(&document)?)
                .is_some()
            {
                return Err(MetricsError::Unavailable);
            }
            if kind == "owner_allocation" {
                let rank = document.get_i64("allocation_rank").map_err(unavailable)?;
                let amount = document
                    .get("allocated_hours")
                    .ok_or(MetricsError::Unavailable)?;
                if chunk != 0
                    || allocations
                        .entry(metric)
                        .or_default()
                        .insert(rank, doc! {"owner":series,"amount":amount.clone()})
                        .is_some()
                {
                    return Err(MetricsError::Unavailable);
                }
            }
        }
        let families: Vec<_> = families
            .into_iter()
            .map(|(kind, (chunks, hash))| {
                doc! {"kind":kind,"chunks":chunks,"sha256":format!("{:x}",hash.finalize())}
            })
            .collect();
        let mut line_index = Document::new();
        for (key, line) in lines {
            line_index.insert(key, Bson::Binary(line.commitment()?));
        }
        let mut order_index = Document::new();
        for (metric, entries) in allocations {
            let mut order = Vec::with_capacity(entries.len());
            for (index, (rank, entry)) in entries.into_iter().enumerate() {
                if rank != index as i64 {
                    return Err(MetricsError::Unavailable);
                }
                order.push(entry);
            }
            order_index.insert(metric.to_string(), order);
        }
        Ok(
            doc! {"version":VERSION,"generation":generation,"chunks":documents.len() as i64,
            "page_bytes":page_bytes,"bytes":0_i64,"sha256":format!("{:x}",digest.finalize()),
            "families":families,"lines":line_index,"allocation_order":order_index},
        )
    }

    pub fn bounded_len(manifest: &Document) -> Result<usize> {
        let bytes = mongodb::bson::serialize_to_vec(manifest)
            .map_err(unavailable)?
            .len();
        if bytes > MAX_MANIFEST_BYTES {
            return Err(MetricsError::Busy);
        }
        Ok(bytes)
    }

    pub fn parse(generation: i64, manifest: &Document) -> Result<Self> {
        if manifest.get_i64("version").ok() != Some(VERSION)
            || manifest.get_i64("generation").ok() != Some(generation)
            || Self::bounded_len(manifest).is_err()
        {
            return Err(MetricsError::Unavailable);
        }
        let mut families = BTreeSet::new();
        let mut chunks = 0_i64;
        let mut committed = 0_i64;
        for entry in manifest.get_array("families").map_err(unavailable)? {
            let entry = entry.as_document().ok_or(MetricsError::Unavailable)?;
            let kind = entry.get_str("kind").map_err(unavailable)?;
            let count = entry.get_i64("chunks").map_err(unavailable)?;
            let digest = entry.get_str("sha256").map_err(unavailable)?;
            if !FAMILIES.contains(&kind)
                || !families.insert(kind.to_owned())
                || count <= 0
                || digest.len() != 64
                || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(MetricsError::Unavailable);
            }
            chunks = chunks.checked_add(count).ok_or(MetricsError::Unavailable)?;
            if kind != "contributor" {
                committed = committed
                    .checked_add(count)
                    .ok_or(MetricsError::Unavailable)?;
            }
        }
        if manifest.get_i64("chunks").ok() != Some(chunks) {
            return Err(MetricsError::Unavailable);
        }
        let lines = manifest.get_document("lines").map_err(unavailable)?;
        let mut indexed = 0_i64;
        for (key, value) in lines {
            let Bson::Binary(value) = value else {
                return Err(MetricsError::Unavailable);
            };
            if key.len() != 64
                || !key.bytes().all(|byte| byte.is_ascii_hexdigit())
                || value.subtype != BinarySubtype::Generic
                || value.bytes.len() != COMMITMENT_BYTES
            {
                return Err(MetricsError::Unavailable);
            }
            let count = u64::from_be_bytes(value.bytes[..8].try_into().map_err(unavailable)?);
            if !(1..=12).contains(&count) {
                return Err(MetricsError::Unavailable);
            }
            indexed = indexed
                .checked_add(count as i64)
                .ok_or(MetricsError::Unavailable)?;
        }
        if indexed != committed {
            return Err(MetricsError::Unavailable);
        }
        Ok(Self {
            generation,
            families,
            lines: lines.clone(),
            allocations: manifest
                .get_document("allocation_order")
                .map_err(unavailable)?
                .clone(),
        })
    }

    pub fn has_family(&self, kind: &str) -> bool {
        self.families.contains(kind)
    }

    pub(super) fn verify_line(
        &self,
        kind: &str,
        metric: i64,
        series: &str,
        count: usize,
        hash: Sha256,
    ) -> Result<bool> {
        let key = line_key(kind, metric, series)?;
        let Some(expected) = self.lines.get(&key) else {
            return if count == 0 {
                Ok(false)
            } else {
                Err(MetricsError::Unavailable)
            };
        };
        let Bson::Binary(expected) = expected else {
            return Err(MetricsError::Unavailable);
        };
        let mut result = Vec::with_capacity(COMMITMENT_BYTES);
        result.extend((count as u64).to_be_bytes());
        result.extend(hash.finalize());
        if count == 0 || expected.subtype != BinarySubtype::Generic || expected.bytes != result {
            return Err(MetricsError::Unavailable);
        }
        Ok(true)
    }

    pub fn verify_document(
        &self,
        kind: &str,
        metric: i64,
        series: &str,
        document: &Document,
    ) -> Result<()> {
        let mut hash = Sha256::new();
        hash.update(document_digest(document)?);
        if !self.verify_line(kind, metric, series, 1, hash)? {
            return Err(MetricsError::Unavailable);
        }
        Ok(())
    }

    pub fn allocation_order(&self, metric: i64) -> Result<&[Bson]> {
        self.allocations
            .get_array(metric.to_string())
            .map(Vec::as_slice)
            .map_err(unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_encoded_size_allows_exact_cap() {
        let base = mongodb::bson::serialize_to_vec(&doc! {"padding":""})
            .unwrap()
            .len();
        let exact = "x".repeat(MAX_MANIFEST_BYTES - base);
        assert_eq!(
            ProjectionManifest::bounded_len(&doc! {"padding": &exact}).unwrap(),
            MAX_MANIFEST_BYTES
        );
        assert!(matches!(
            ProjectionManifest::bounded_len(&doc! {"padding": format!("{exact}x")}),
            Err(MetricsError::Busy)
        ));
    }

    #[test]
    fn stored_field_order_preserves_projection_commitment() {
        let original = doc! {"report_id":"fixture","generation":1_i64,"kind":"chart_total","metric":0_i64,"series":"","chunk":0_i64,"_id":"fixture-page"};
        let mut stored = doc! {"_id":"fixture-page"};
        stored.extend(original.clone());
        let raw = RawDocumentBuf::try_from(&original).unwrap();
        let manifest =
            ProjectionManifest::build(1, std::slice::from_ref(&raw), raw.as_bytes().len() as i64)
                .unwrap();
        let parsed = ProjectionManifest::parse(1, &manifest).unwrap();
        assert!(parsed.has_family("chart_total"));
        parsed
            .verify_document("chart_total", 0, "", &stored)
            .unwrap();
    }

    #[test]
    fn allocation_order_is_bound_to_published_ranks() {
        let pages = [
            doc! {"generation":2_i64,"kind":"owner_allocation","metric":0_i64,"series":"b","chunk":0_i64,"allocation_rank":1_i64,"allocated_hours":3.0},
            doc! {"generation":2_i64,"kind":"owner_allocation","metric":0_i64,"series":"a","chunk":0_i64,"allocation_rank":0_i64,"allocated_hours":4.0},
        ];
        let raw: Vec<_> = pages
            .iter()
            .map(|doc| RawDocumentBuf::try_from(doc).unwrap())
            .collect();
        let bytes = raw.iter().map(|doc| doc.as_bytes().len() as i64).sum();
        let manifest = ProjectionManifest::build(2, &raw, bytes).unwrap();
        let parsed = ProjectionManifest::parse(2, &manifest).unwrap();
        let order = parsed.allocation_order(0).unwrap();
        assert_eq!(
            order[0].as_document().unwrap().get_str("owner").unwrap(),
            "a"
        );
        assert_eq!(
            order[1].as_document().unwrap().get_str("owner").unwrap(),
            "b"
        );
        for page in &pages {
            parsed
                .verify_document("owner_allocation", 0, page.get_str("series").unwrap(), page)
                .unwrap();
        }
    }
}

use super::*;
use layer3_adapter::{
    archive_source::{Kind, RestoreTree},
    permanent_custody::{digest, ObjectReceipt, PermanentCustody},
};
use sha2::{Digest, Sha256};
use std::os::unix::fs::OpenOptionsExt;
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    path::Path,
};

pub(crate) async fn load(
    custody: &dyn PermanentCustody,
    receipt: &ObjectReceipt,
    request: &Request,
    bytes: &[u8],
) -> Result<Manifest> {
    let data = custody.read(receipt).await?;
    let manifest: Manifest = serde_json::from_slice(&data).map_err(|_| REFUSED)?;
    if manifest.format != 1
        || manifest.operation_id != request.operation_id
        || manifest.request_sha256 != digest(bytes)
        || custody.read(&manifest.original_request).await? != bytes
        || digest(&custody.read(&manifest.original_source).await?) != request.source_sha256
        || digest(&custody.read(&manifest.exclusion).await?) != request.exclusion_sha256
        || manifest.custody != custody.policy().await?
        || manifest.schema_sha256 != crate::store::retirement_archive::schema_sha256()
    {
        return Err(REFUSED);
    }
    Ok(manifest)
}
pub(crate) async fn restore(
    custody: &dyn PermanentCustody,
    receipt: &ObjectReceipt,
    request: &Request,
    bytes: &[u8],
    destination: &Path,
) -> Result<Verification> {
    let manifest = load(custody, receipt, request, bytes).await?;
    let root = RestoreTree::create(destination)?;
    let mut result = Verification {
        format: 1,
        operation_id: request.operation_id.clone(),
        manifest: receipt.clone(),
        custody_sha256: manifest.custody.sha256()?,
        objects: 3,
        files: 0,
        documents: 0,
        bytes: 0,
        restore_sha256: String::new(),
    };
    let mut proof = Sha256::new();
    for role in &request.roots {
        let mut tree = RestoreTree::create(&root.path().join(&role.role))?;
        let records = manifest
            .files
            .iter()
            .filter(|record| record.role == role.role)
            .collect::<Vec<_>>();
        if records.is_empty() {
            return Err(REFUSED);
        }
        for record in records {
            let mut output = tree.add(&record.entry)?;
            let mut hash = Sha256::new();
            let mut length = 0;
            if record.entry.kind != Kind::File && !record.objects.is_empty() {
                return Err(REFUSED);
            }
            for object in &record.objects {
                let data = custody.read(object).await?;
                hash.update(&data);
                length += data.len() as u64;
                if let Some(file) = output.as_mut() {
                    RestoreTree::write_chunk(file, &data)?;
                }
                result.objects += 1;
                result.bytes += data.len() as u64;
            }
            if format!("{:x}", hash.finalize()) != record.sha256 {
                return Err(REFUSED);
            }
            if record.entry.kind == Kind::File {
                if length != record.entry.metadata.length {
                    return Err(REFUSED);
                }
                if let Some(file) = output {
                    file.sync_all().map_err(|_| REFUSED)?;
                }
                tree.verify_file(&record.entry, &record.sha256)?;
            }
            result.files += 1;
            proof.update(serde_json::to_vec(record).map_err(|_| REFUSED)?);
        }
        tree.finish()?;
    }
    if manifest
        .files
        .iter()
        .any(|r| !request.roots.iter().any(|root| root.role == r.role))
    {
        return Err(REFUSED);
    }
    let db = RestoreTree::create(&root.path().join("database"))?;
    let mut names = BTreeSet::new();
    for record in &manifest.database {
        if !crate::store::isolation::COLLECTIONS.contains(&record.collection.as_str())
            || !names.insert(&record.collection)
        {
            return Err(REFUSED);
        }
        let path = db.path().join(format!("{}.bson", record.collection));
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| REFUSED)?;
        let mut hash = Sha256::new();
        let mut length = 0;
        let mut count = 0;
        let mut ids = BTreeSet::new();
        for object in &record.objects {
            let data = custody.read(object).await?;
            count += validate_bson(&data, &mut ids)?;
            hash.update(&data);
            length += data.len() as u64;
            output.write_all(&data).map_err(|_| REFUSED)?;
            result.objects += 1;
        }
        output.sync_all().map_err(|_| REFUSED)?;
        drop(output);
        if count != record.documents
            || length != record.bytes
            || format!("{:x}", hash.finalize()) != record.sha256
        {
            return Err(REFUSED);
        }
        verify_reconstruction(&path, &record.sha256, record.bytes)?;
        result.documents += count;
        result.bytes += length;
        proof.update(serde_json::to_vec(record).map_err(|_| REFUSED)?);
    }
    if names.len() != crate::store::isolation::COLLECTIONS.len() {
        return Err(REFUSED);
    }
    let external = RestoreTree::create(&root.path().join("external"))?;
    for (index, record) in manifest.external.iter().enumerate() {
        let data = custody.read(&record.object).await?;
        if digest(&data) != record.sha256 {
            return Err(REFUSED);
        }
        let path = external.path().join(index.to_string());
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| REFUSED)?;
        file.write_all(&data)
            .and_then(|()| file.sync_all())
            .map_err(|_| REFUSED)?;
        verify_reconstruction(&path, &record.sha256, data.len() as u64)?;
        result.objects += 1;
        result.bytes += data.len() as u64;
        proof.update(serde_json::to_vec(record).map_err(|_| REFUSED)?);
    }
    // Retain original metadata (including owners/modes) without making restored
    // code executable or granting setuid permissions in the verification area.
    let metadata = serde_json::to_vec(&manifest).map_err(|_| REFUSED)?;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(root.path().join("manifest.json"))
        .map_err(|_| REFUSED)?;
    file.write_all(&metadata)
        .and_then(|()| file.sync_all())
        .map_err(|_| REFUSED)?;
    result.restore_sha256 = format!("{:x}", proof.finalize());
    if manifest.custody != custody.policy().await? {
        return Err(REFUSED);
    }
    Ok(result)
}
fn verify_reconstruction(path: &Path, expected: &str, length: u64) -> Result<()> {
    let mut file = std::fs::File::open(path).map_err(|_| REFUSED)?;
    let mut hash = Sha256::new();
    let mut count = 0;
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let n = file.read(&mut buffer).map_err(|_| REFUSED)?;
        if n == 0 {
            break;
        }
        count += n as u64;
        hash.update(&buffer[..n]);
    }
    if count != length || format!("{:x}", hash.finalize()) != expected {
        return Err(REFUSED);
    }
    Ok(())
}
pub(crate) fn validate_bson(data: &[u8], ids: &mut BTreeSet<Vec<u8>>) -> Result<u64> {
    let mut offset = 0;
    let mut count = 0;
    while offset < data.len() {
        let size = i32::from_le_bytes(
            data.get(offset..offset + 4)
                .ok_or(REFUSED)?
                .try_into()
                .map_err(|_| REFUSED)?,
        );
        if size < 5 {
            return Err(REFUSED);
        }
        let document: mongodb::bson::Document = mongodb::bson::deserialize_from_slice(
            data.get(offset..offset + size as usize).ok_or(REFUSED)?,
        )
        .map_err(|_| REFUSED)?;
        let id = document.get("_id").ok_or(REFUSED)?.clone();
        let key = mongodb::bson::serialize_to_vec(&mongodb::bson::doc! {"_id":id})
            .map_err(|_| REFUSED)?;
        if !ids.insert(key) {
            return Err(REFUSED);
        }
        offset += size as usize;
        count += 1;
    }
    Ok(count)
}

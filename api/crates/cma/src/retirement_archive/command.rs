use super::*;
use crate::store::retirement_archive::ArchiveStore;
use layer2_core::{
    document_artifacts::DocumentBlobStore, sandbox::retirement_archive::RetirementArchiveReceipt,
};
use layer3_adapter::{
    archive_source::{publish_private_file, read_private_file},
    permanent_custody::{digest, CustodyPolicy, GcsCustody, ObjectReceipt, PermanentCustody},
};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Invocation {
    #[serde(default)]
    enabled: bool,
    request_file: PathBuf,
    output_file: PathBuf,
    #[serde(default)]
    custody: Option<Custody>,
    #[serde(default)]
    restore_directory: Option<PathBuf>,
    #[serde(default)]
    manifest_reference: Option<ObjectReceipt>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Custody {
    bucket: String,
    #[serde(default)]
    create_token_file: Option<PathBuf>,
    verify_token_file: PathBuf,
}
pub(crate) fn is_command(args: &[String]) -> bool {
    args.first().is_some_and(|s| s == "--retirement-archive")
}
pub(crate) async fn command(args: &[String]) -> Result<String> {
    if args.len() != 3 || !is_command(args) {
        return Err(REFUSED);
    }
    let invocation: Invocation = serde_json::from_slice(&read_private_file(
        std::path::Path::new(&args[2]),
        1024 * 1024,
    )?)
    .map_err(|_| REFUSED)?;
    let action = args[1].as_str();
    if ![
        "inspect",
        "dry-run",
        "hold",
        "capture",
        "restore-check",
        "complete",
        "readback",
    ]
    .contains(&action)
        || (!invocation.enabled && !["inspect", "dry-run"].contains(&action))
    {
        return Err(REFUSED);
    }
    let bytes = read_private_file(&invocation.request_file, 1024 * 1024)?;
    let request: Request = serde_json::from_slice(&bytes).map_err(|_| REFUSED)?;
    validate_paths(&invocation, &request, std::path::Path::new(&args[2]))?;
    if action == "readback" {
        request.validate()?;
        let config = invocation.custody.ok_or(REFUSED)?;
        if config.create_token_file.is_some() {
            return Err(REFUSED);
        }
        let custody = GcsCustody::new(
            config.bucket,
            request.operation_id.clone(),
            None,
            config.verify_token_file,
        )?;
        let result = verify::restore(
            &custody,
            &invocation.manifest_reference.ok_or(REFUSED)?,
            &request,
            &bytes,
            &invocation.restore_directory.ok_or(REFUSED)?,
        )
        .await?;
        return publish(&invocation.output_file, &result);
    }
    let name = std::env::var("CHRONO_MONGODB_DATABASE").map_err(|_| REFUSED)?;
    let mut options = mongodb::options::ClientOptions::parse(
        std::env::var("CHRONO_MONGODB_URI").map_err(|_| REFUSED)?,
    )
    .await
    .map_err(|_| REFUSED)?;
    crate::store::transaction_barrier::bind(&mut options, &name);
    let client = mongodb::Client::with_options(options).map_err(|_| REFUSED)?;
    let database = client.database(&name);
    let store = ArchiveStore { client, database };
    if action == "inspect" {
        let current = store.inspect(&request.sandbox_id).await?;
        let source = mongodb::bson::serialize_to_vec(&current).map_err(|_| REFUSED)?;
        let intent=store.database.collection::<mongodb::bson::Document>("sandbox_events")
            .find_one(mongodb::bson::doc!{"sandbox_id":&request.sandbox_id,"record.kind.type":"status_changed","record.kind.to":"retiring"})
            .sort(mongodb::bson::doc!{"seq":-1}).await.map_err(|_|REFUSED)?.ok_or(REFUSED)?;
        let evidence = serde_json::json!({"format":1,"source_sha256":digest(&source),"revision":current.get_i64("revision").map_err(|_|REFUSED)?,
            "retirement_intent_sha256":digest(&mongodb::bson::serialize_to_vec(&intent).map_err(|_|REFUSED)?),
            "schema_sha256":crate::store::retirement_archive::schema_sha256()});
        return publish(&invocation.output_file, &evidence);
    }
    request.validate()?;
    if action != "complete" {
        export::exclusion(&request)?;
    }
    if action == "hold" {
        store.admit(&request, &bytes).await?;
        return publish(
            &invocation.output_file,
            &serde_json::json!({"format":1,"held":true,"request_sha256":digest(&bytes)}),
        );
    }
    let custody: Box<dyn PermanentCustody> = if action == "dry-run" {
        Box::new(DigestOnly)
    } else {
        let config = invocation.custody.ok_or(REFUSED)?;
        Box::new(GcsCustody::new(
            config.bucket,
            request.operation_id.clone(),
            config.create_token_file,
            config.verify_token_file,
        )?)
    };
    let blobs: Option<Arc<dyn DocumentBlobStore>> = if ["capture", "dry-run", "complete"]
        .contains(&action)
    {
        crate::config::DocumentStorageConfig::from_lookup(&mut |name| Ok(std::env::var(name).ok()))
            .map_err(|_| REFUSED)?
            .map(crate::document_storage::StorageDocuments::read_only)
            .transpose()
            .map_err(|_| REFUSED)?
            .map(|s| s as Arc<dyn DocumentBlobStore>)
    } else {
        None
    };
    match action {
        "dry-run" => {
            let manifest =
                export::build(&store, &request, &bytes, custody.as_ref(), blobs.as_deref()).await?;
            publish(
                &invocation.output_file,
                &serde_json::json!({"format":1,"dry_run":true,"apply_authority":false,"custody_verified":false,
                "files":manifest.files.len(),"documents":manifest.database.iter().map(|r|r.documents).sum::<u64>(),
                "external_objects":manifest.external.len(),"inventory_sha256":digest(&serde_json::to_vec(&manifest).map_err(|_|REFUSED)?)}),
            )
        }
        "capture" => {
            let receipt =
                export::publish(&store, &request, &bytes, custody.as_ref(), blobs.as_deref())
                    .await?;
            publish(&invocation.output_file, &receipt)
        }
        "restore-check" => {
            let manifest = store.manifest(&request, &bytes).await?;
            let verification = verify::restore(
                custody.as_ref(),
                &manifest,
                &request,
                &bytes,
                &invocation.restore_directory.ok_or(REFUSED)?,
            )
            .await?;
            let receipt = custody
                .retain(
                    &request.operation_id,
                    serde_json::to_vec(&verification).map_err(|_| REFUSED)?,
                )
                .await?;
            store.retain_restore(&request, &bytes, &receipt).await?;
            publish(&invocation.output_file, &verification)
        }
        "complete" => {
            let manifest_receipt = store.manifest(&request, &bytes).await?;
            let manifest =
                verify::load(custody.as_ref(), &manifest_receipt, &request, &bytes).await?;
            let restore_receipt = store.restore(&request, &bytes).await?;
            let verification: Verification =
                serde_json::from_slice(&custody.read(&restore_receipt).await?)
                    .map_err(|_| REFUSED)?;
            if verification.format != 1
                || verification.operation_id != request.operation_id
                || verification.manifest != manifest_receipt
                || verification.custody_sha256 != manifest.custody.sha256()?
                || !hash(&verification.restore_sha256)
            {
                return Err(REFUSED);
            }
            let current = store.inspect(&request.sandbox_id).await?;
            let parent = crate::store::retirement_archive::accepted(&current, &request, &bytes)?;
            if parent.retirement_archive_pending()
                && !crate::store::retirement_archive::matches_current(
                    &manifest,
                    export::build(&store, &request, &bytes, custody.as_ref(), blobs.as_deref())
                        .await?,
                )?
            {
                return Err(REFUSED);
            }
            let receipt = RetirementArchiveReceipt {
                manifest_sha256: manifest_receipt.sha256,
                custody_sha256: verification.custody_sha256,
                verification_sha256: restore_receipt.sha256,
            };
            store.complete(&request, &bytes, receipt).await?;
            publish(
                &invocation.output_file,
                &serde_json::json!({"format":1,"verified_archive":true,"source_deletion_performed":false,"request_sha256":digest(&bytes)}),
            )
        }
        _ => Err(REFUSED),
    }
}
fn validate_paths(
    invocation: &Invocation,
    request: &Request,
    invocation_file: &std::path::Path,
) -> Result<()> {
    let mut paths = vec![
        invocation_file,
        invocation.request_file.as_path(),
        invocation.output_file.as_path(),
        request.exclusion_receipt.as_path(),
    ];
    if let Some(path) = &invocation.restore_directory {
        paths.push(path);
    }
    if let Some(custody) = &invocation.custody {
        paths.push(&custody.verify_token_file);
        if let Some(path) = &custody.create_token_file {
            paths.push(path);
        }
    }
    for path in paths {
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(REFUSED);
        }
        if request
            .roots
            .iter()
            .any(|root| path.starts_with(&root.path))
        {
            return Err(REFUSED);
        }
    }
    Ok(())
}
fn publish<T: serde::Serialize>(path: &std::path::Path, value: &T) -> Result<String> {
    if !path.is_absolute() {
        return Err(REFUSED);
    }
    let bytes = serde_json::to_vec(value).map_err(|_| REFUSED)?;
    publish_private_file(path, &bytes).map_err(|_| REFUSED)?;
    Ok(serde_json::json!({"format":1,"evidence_sha256":digest(&bytes),"evidence_bytes":bytes.len()}).to_string())
}
struct DigestOnly;
#[async_trait::async_trait]
impl PermanentCustody for DigestOnly {
    async fn policy(&self) -> Result<CustodyPolicy> {
        Ok(CustodyPolicy {
            format: 1,
            bucket: "dry-run".into(),
            metageneration: "0".into(),
            permanent_hold: false,
            private_uniform_access: false,
            independent_read_only: false,
        })
    }
    async fn retain(&self, operation: &str, bytes: Vec<u8>) -> Result<ObjectReceipt> {
        let sha = digest(&bytes);
        Ok(ObjectReceipt {
            key: format!("retirement-v1/{operation}/{sha}"),
            generation: "0".into(),
            sha256: sha,
            bytes: bytes.len() as u64,
        })
    }
    async fn read(&self, _: &ObjectReceipt) -> Result<Vec<u8>> {
        Err(REFUSED)
    }
}

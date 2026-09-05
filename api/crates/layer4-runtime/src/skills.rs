use std::io::Cursor;
use std::net::IpAddr;
use std::path::Path;

use futures_util::StreamExt as _;
use layer3_adapter::AgentSkill;
use sha2::{Digest, Sha256};

use crate::command::hex;
use crate::failure::{Result, RuntimeError};
use crate::paths::{safe_component, safe_relative};

const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXPANDED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 4_096;

pub(crate) async fn install(
    client: &reqwest::Client,
    nyxid_url: Option<&str>,
    skill: &AgentSkill,
    destination: &Path,
) -> Result<()> {
    safe_component(&skill.id, "skill id")?;
    require_sha256(&skill.archive_sha256)?;
    let base = nyxid_url.ok_or_else(|| {
        RuntimeError::unavailable(
            "skill_unavailable",
            format!(
                "skill {} cannot be fetched without the NyxID loopback proxy",
                skill.id
            ),
        )
    })?;
    let target = proxied_url(base, &skill.source_url)?;
    let archive = download(client, target, &skill.id).await?;
    let actual = hex(&Sha256::digest(&archive));
    if !actual.eq_ignore_ascii_case(&skill.archive_sha256) {
        return Err(RuntimeError::rejected(
            "skill_checksum_mismatch",
            format!("skill {} archive did not match its SHA-256", skill.id),
        ));
    }
    let destination = destination.to_path_buf();
    tokio::task::spawn_blocking(move || extract(&archive, &destination))
        .await
        .map_err(|error| {
            RuntimeError::unavailable(
                "skill_extract_failed",
                format!("skill extraction task failed: {error}"),
            )
        })??;
    Ok(())
}

async fn download(client: &reqwest::Client, url: url::Url, id: &str) -> Result<Vec<u8>> {
    let response = client.get(url).send().await.map_err(|error| {
        RuntimeError::unavailable(
            "skill_unavailable",
            format!("fetch skill {id} through the loopback proxy: {error}"),
        )
    })?;
    if !response.status().is_success() {
        return Err(RuntimeError::unavailable(
            "skill_unavailable",
            format!(
                "fetch skill {id}: proxy returned HTTP {}",
                response.status()
            ),
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ARCHIVE_BYTES as u64)
    {
        return Err(RuntimeError::rejected(
            "skill_archive_too_large",
            format!("skill {id} archive exceeds {MAX_ARCHIVE_BYTES} bytes"),
        ));
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            RuntimeError::unavailable(
                "skill_unavailable",
                format!("read skill {id} archive: {error}"),
            )
        })?;
        if bytes.len().saturating_add(chunk.len()) > MAX_ARCHIVE_BYTES {
            return Err(RuntimeError::rejected(
                "skill_archive_too_large",
                format!("skill {id} archive exceeds {MAX_ARCHIVE_BYTES} bytes"),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn proxied_url(base: &str, source: &str) -> Result<url::Url> {
    let mut proxy = url::Url::parse(base).map_err(|_| invalid_source())?;
    let loopback = proxy
        .host_str()
        .and_then(|host| host.parse::<IpAddr>().ok())
        .is_some_and(|address| address.is_loopback());
    if proxy.scheme() != "http"
        || !loopback
        || !proxy.username().is_empty()
        || proxy.password().is_some()
        || proxy.query().is_some()
        || proxy.fragment().is_some()
    {
        return Err(RuntimeError::rejected(
            "skill_proxy_invalid",
            "NYXID_URL must be an uncredentialed HTTP loopback URL",
        ));
    }
    let source = url::Url::parse(source)
        .or_else(|_| proxy.join(source))
        .map_err(|_| invalid_source())?;
    let path = source.path();
    if crate::upstream_target::unsafe_path(path) || source.fragment().is_some() {
        return Err(invalid_source());
    }
    let path = if path.starts_with("/api/v1/proxy/s/ornn-api/") {
        path.to_string()
    } else {
        format!("/api/v1/proxy/s/ornn-api/{}", path.trim_start_matches('/'))
    };
    proxy.set_path(&path);
    proxy.set_query(source.query());
    Ok(proxy)
}

fn extract(bytes: &[u8], destination: &Path) -> Result<()> {
    crate::fs_durability::create_dir(destination).map_err(extract_io("create skill directory"))?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| {
        RuntimeError::rejected("skill_archive_invalid", format!("open skill ZIP: {error}"))
    })?;
    if archive.len() > MAX_ENTRIES {
        return Err(RuntimeError::rejected(
            "skill_archive_invalid",
            format!("skill ZIP contains more than {MAX_ENTRIES} entries"),
        ));
    }
    let mut expanded = 0_u64;
    let mut seen = std::collections::BTreeSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            RuntimeError::rejected("skill_archive_invalid", format!("read skill ZIP: {error}"))
        })?;
        if entry.is_symlink() {
            return Err(RuntimeError::rejected(
                "skill_archive_invalid",
                "skill ZIP contains a symbolic link",
            ));
        }
        let enclosed = entry.enclosed_name().ok_or_else(|| {
            RuntimeError::rejected("skill_archive_invalid", "skill ZIP path escapes its root")
        })?;
        if enclosed.as_os_str().is_empty() {
            continue;
        }
        let relative = safe_relative(
            enclosed.to_str().ok_or_else(|| {
                RuntimeError::rejected("skill_archive_invalid", "skill ZIP path is not UTF-8")
            })?,
            "skill ZIP entry",
        )?;
        if !seen.insert(relative.clone()) {
            return Err(RuntimeError::rejected(
                "skill_archive_invalid",
                "skill ZIP contains duplicate paths",
            ));
        }
        let path = destination.join(relative);
        if entry.is_dir() {
            crate::fs_durability::create_dir_all(&path)
                .map_err(extract_io("create skill subdirectory"))?;
            continue;
        }
        expanded = expanded
            .checked_add(entry.size())
            .ok_or_else(expanded_error)?;
        if expanded > MAX_EXPANDED_BYTES {
            return Err(expanded_error());
        }
        let parent = path.parent().expect("a skill file has a parent");
        crate::fs_durability::create_dir_all(parent)
            .map_err(extract_io("create skill file parent"))?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(extract_io("create skill file"))?;
        std::io::copy(&mut entry, &mut output).map_err(extract_io("extract skill file"))?;
        set_mode(&output, entry.unix_mode().unwrap_or(0o644) & 0o777)?;
        output.sync_all().map_err(extract_io("sync skill file"))?;
        drop(output);
        crate::fs_durability::sync_parent(&path).map_err(extract_io("sync skill file parent"))?;
    }
    Ok(())
}

fn require_sha256(value: &str) -> Result<()> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(RuntimeError::rejected(
            "skill_checksum_invalid",
            "skill archive SHA-256 must contain 64 hexadecimal characters",
        ))
    }
}

#[cfg(unix)]
fn set_mode(file: &std::fs::File, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    file.set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(extract_io("set skill file mode"))
}

#[cfg(not(unix))]
fn set_mode(_file: &std::fs::File, _mode: u32) -> Result<()> {
    Ok(())
}

fn invalid_source() -> RuntimeError {
    RuntimeError::rejected("skill_source_invalid", "skill source URL is invalid")
}

fn expanded_error() -> RuntimeError {
    RuntimeError::rejected(
        "skill_archive_too_large",
        format!("expanded skill exceeds {MAX_EXPANDED_BYTES} bytes"),
    )
}

fn extract_io(operation: &'static str) -> impl FnOnce(std::io::Error) -> RuntimeError + Copy {
    move |error| RuntimeError::unavailable("skill_extract_failed", format!("{operation}: {error}"))
}

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;

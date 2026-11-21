//! Typed documents use the shared complete-byte classifier; opaque files stay opaque.
use super::Error;
use crate::whatsapp::outbound::document::{self, Document};
use cmaeg_core::message::NativeAttachment;

pub(super) struct Profile {
    pub mime: &'static str,
    pub file_type: &'static str,
    /// Empty means the supplied filename is opaque, not a claimed document type.
    pub extensions: &'static [&'static str],
}

pub(super) fn profile(item: &NativeAttachment, bytes: &[u8]) -> Result<Profile, Error> {
    // The existing public File contract supports arbitrary byte payloads. An explicit
    // generic MIME never claims document validity and must not cause transcoding.
    let binary = Profile {
        mime: "application/octet-stream",
        file_type: "stream",
        extensions: &[],
    };
    if item.mime_type.as_deref() == Some(binary.mime) {
        return Ok(binary);
    }
    if item.mime_type.as_deref() != Some("application/zip") {
        if let Some(kind) = document::inspect(bytes) {
            return Ok(Profile {
                mime: kind.mime(),
                file_type: match kind {
                    Document::Pdf => "pdf",
                    Document::Word => "doc",
                    Document::Excel => "xls",
                    Document::PowerPoint => "ppt",
                    // The regional enum lists legacy Office only. Modern Office
                    // packages use its explicitly documented generic stream route.
                    _ => "stream",
                },
                extensions: kind.extensions(),
            });
        }
    }
    if bytes.starts_with(b"PK") && document::zip_archive(bytes) {
        return Ok(Profile {
            mime: "application/zip",
            file_type: "stream",
            extensions: &["zip"],
        });
    }
    if item.mime_type.is_none() {
        Ok(binary)
    } else {
        Err(Error::Invalid)
    }
}

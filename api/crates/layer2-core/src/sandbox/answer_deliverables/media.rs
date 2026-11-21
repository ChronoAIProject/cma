use quick_xml::{events::Event, Reader};
use serde::Deserialize;
use std::io::{Cursor, Read};
use std::path::Path;
use zip::ZipArchive;

pub(super) const OPAQUE: &str = "application/octet-stream";
const MAX_OFFICE_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy)]
enum NativeFormat {
    Word,
    Spreadsheet,
    Csv,
    Zip,
    Json,
}

impl NativeFormat {
    fn for_path(path: &str) -> Option<Self> {
        match Path::new(path)
            .extension()?
            .to_str()?
            .to_ascii_lowercase()
            .as_str()
        {
            "docx" => Some(Self::Word),
            "xlsx" => Some(Self::Spreadsheet),
            "csv" => Some(Self::Csv),
            "zip" => Some(Self::Zip),
            "json" => Some(Self::Json),
            _ => None,
        }
    }

    const fn media_type(self) -> &'static str {
        match self {
            Self::Word => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            Self::Spreadsheet => {
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            }
            Self::Csv => "text/csv",
            Self::Zip => "application/zip",
            Self::Json => "application/json",
        }
    }

    fn matches(self, bytes: &[u8]) -> bool {
        match self {
            Self::Word => office_package(
                bytes,
                "word/document.xml",
                b"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
            ),
            Self::Spreadsheet => office_package(
                bytes,
                "xl/workbook.xml",
                b"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
            ),
            Self::Zip => archive(bytes).is_some(),
            Self::Csv => csv(bytes),
            Self::Json => json(bytes),
        }
    }
}

/// Validate the shape of a stored marker. The bytes are verified independently.
pub(super) fn allowed(path: &str, media_type: &str) -> bool {
    media_type == OPAQUE
        || NativeFormat::for_path(path).is_some_and(|kind| kind.media_type() == media_type)
}

/// Classify bounded, authorized bytes. Returned types are only for attachments.
pub(super) fn verified(path: &str, bytes: &[u8]) -> &'static str {
    NativeFormat::for_path(path)
        .filter(|kind| kind.matches(bytes))
        .map_or(OPAQUE, NativeFormat::media_type)
}

fn archive(bytes: &[u8]) -> Option<ZipArchive<Cursor<&[u8]>>> {
    if !matches!(bytes.get(..4), Some(b"PK\x03\x04" | b"PK\x05\x06")) {
        return None;
    }
    ZipArchive::new(Cursor::new(bytes)).ok()
}

fn office_package(bytes: &[u8], entry: &str, content_type: &[u8]) -> bool {
    let Some(mut zip) = archive(bytes) else {
        return false;
    };
    if zip.index_for_name(entry).is_none() || zip.index_for_name("_rels/.rels").is_none() {
        return false;
    }
    let Ok(manifest) = zip.by_name("[Content_Types].xml") else {
        return false;
    };
    if manifest.size() > MAX_OFFICE_MANIFEST_BYTES {
        return false;
    }
    // Inspect the package manifest without extracting files or reading the document body.
    let mut xml = Vec::new();
    if manifest
        .take(MAX_OFFICE_MANIFEST_BYTES + 1)
        .read_to_end(&mut xml)
        .is_err()
        || xml.len() as u64 > MAX_OFFICE_MANIFEST_BYTES
    {
        return false;
    }
    let mut reader = Reader::from_reader(xml.as_slice());
    let mut matched = false;
    loop {
        match reader.read_event() {
            Ok(Event::Empty(element) | Event::Start(element))
                if element.local_name().as_ref() == b"Override" =>
            {
                let Ok(Some(part)) = element.try_get_attribute("PartName") else {
                    return false;
                };
                if part.value.strip_prefix(b"/") != Some(entry.as_bytes()) {
                    continue;
                }
                let Ok(Some(declared)) = element.try_get_attribute("ContentType") else {
                    return false;
                };
                if matched || declared.value.as_ref() != content_type {
                    return false;
                }
                matched = true;
            }
            Ok(Event::Eof) => return matched,
            Ok(Event::DocType(_)) | Err(_) => return false,
            _ => {}
        }
    }
}

fn csv(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let visible = text.trim_start_matches('\u{feff}').trim_start();
    if visible.starts_with('<')
        || (visible.starts_with(['{', '[']) && json(visible.as_bytes()))
        || text
            .chars()
            .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
    {
        return false;
    }
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(bytes);
    let mut records = reader.byte_records();
    matches!(records.next(), Some(Ok(_))) && records.all(|record| record.is_ok())
}

fn json(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut reader = serde_json::Deserializer::from_str(text);
    serde::de::IgnoredAny::deserialize(&mut reader).is_ok() && reader.end().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::{write::SimpleFileOptions, ZipWriter};

    #[test]
    fn verifies_csv_and_json_bytes_with_opaque_fallback() {
        for (path, bytes, expected) in [
            (
                "nested/Report.CSV",
                b"name,value\r\n\"quoted, entry\",9\r\n".as_slice(),
                "text/csv",
            ),
            ("one-column.csv", b"heading\nvalue", "text/csv"),
            ("data.json", br#"{"value":9}"#, "application/json"),
            ("data.json", b"{} {}", OPAQUE),
            ("report.csv", br#"{"value":9,"other":1}"#, OPAQUE),
            ("report.csv", b"<html>fake,3</html>", OPAQUE),
            ("report.csv", b"name,value\nno value here\n", OPAQUE),
            ("report.csv", b"head\0er", OPAQUE),
            ("files.csv/entry", b"name,value", OPAQUE),
            ("file.zip", b"PK\x03\x04wrong", OPAQUE),
        ] {
            assert_eq!(verified(path, bytes), expected, "{path}: {bytes:?}");
        }
        assert!(allowed("legacy/report.zip", OPAQUE));
        assert!(allowed("reports/April.ZIP", "application/zip"));
        assert!(!allowed("reports/April.json", "application/zip"));
    }

    #[test]
    fn requires_an_office_package_with_the_matching_main_part() {
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in [
            ("word/document.xml", b"<test/>".as_slice()),
            ("_rels/.rels", b"<Relationships/>"),
            ("[Content_Types].xml", br#"<Types><Override PartName="/word/document.xml" ContentType="application/vnd.ms-word.document.macroEnabled.main+xml"/></Types>"#),
        ] {
            archive.start_file(name, SimpleFileOptions::default()).unwrap();
            archive.write_all(bytes).unwrap();
        }
        let bytes = archive.finish().unwrap().into_inner();
        assert_eq!(verified("macro.docx", &bytes), OPAQUE);
        assert_eq!(verified("wrong.xlsx", &bytes), OPAQUE);
        assert_eq!(verified("archive.zip", &bytes), "application/zip");
    }
}

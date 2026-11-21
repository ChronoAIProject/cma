use quick_xml::{
    events::{BytesStart, Event},
    name::ResolveResult,
    NsReader,
};
use serde::Deserialize;
use std::io::{Cursor, Read};
use std::path::Path;
use zip::ZipArchive;

pub(super) const OPAQUE: &str = "application/octet-stream";
const MAX_OFFICE_XML_BYTES: u64 = 64 * 1024;
const RELATIONSHIPS_NAMESPACE: &[u8] =
    b"http://schemas.openxmlformats.org/package/2006/relationships";
const CONTENT_TYPES_NAMESPACE: &[u8] =
    b"http://schemas.openxmlformats.org/package/2006/content-types";

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
                b"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
            ),
            Self::Spreadsheet => office_package(
                bytes,
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

fn office_package(bytes: &[u8], content_type: &[u8]) -> bool {
    let Some(mut zip) = archive(bytes) else {
        return false;
    };
    let Some(relationships) = read_xml_part(&mut zip, "_rels/.rels") else {
        return false;
    };
    let Some(entry) = office_document_target(&relationships) else {
        return false;
    };
    if zip
        .index_for_name(&entry)
        .and_then(|index| zip.by_index_raw(index).ok())
        .is_none_or(|part| !part.is_file())
    {
        return false;
    }
    read_xml_part(&mut zip, "[Content_Types].xml")
        .is_some_and(|manifest| declares_content_type(&manifest, &entry, content_type))
}

fn read_xml_part(zip: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let part = zip.by_name(name).ok()?;
    if part.size() > MAX_OFFICE_XML_BYTES {
        return None;
    }
    // Inspect only the bounded metadata parts without extracting the document.
    let mut xml = Vec::new();
    part.take(MAX_OFFICE_XML_BYTES + 1)
        .read_to_end(&mut xml)
        .ok()?;
    (xml.len() as u64 <= MAX_OFFICE_XML_BYTES).then_some(xml)
}

fn office_document_target(xml: &[u8]) -> Option<String> {
    let mut target = None;
    package_children(xml, b"Relationships", RELATIONSHIPS_NAMESPACE, |element| {
        if element.local_name().as_ref() != b"Relationship" {
            return Some(());
        }
        let relation = element.try_get_attribute("Type").ok()??;
        if !matches!(
            relation.value.as_ref(),
            b"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"
                | b"http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument"
        ) {
            return Some(());
        }
        if target.is_some()
            || element
                .try_get_attribute("TargetMode")
                .ok()?
                .is_some_and(|mode| mode.value.as_ref() != b"Internal")
        {
            return None;
        }
        let entry = element.try_get_attribute("Target").ok()??;
        let path = std::str::from_utf8(&entry.value).ok()?;
        let path = path.strip_prefix('/').unwrap_or(path);
        if path.is_empty()
            || path.contains([':', '?', '#', '%', '&'])
            || !layer3_adapter::valid_agent_file_path(path)
        {
            return None;
        }
        target = Some(path.to_owned());
        Some(())
    })?;
    target
}

fn declares_content_type(xml: &[u8], entry: &str, content_type: &[u8]) -> bool {
    let mut matched = false;
    let parsed = package_children(xml, b"Types", CONTENT_TYPES_NAMESPACE, |element| {
        if element.local_name().as_ref() != b"Override" {
            return Some(());
        }
        let part = element.try_get_attribute("PartName").ok()??;
        if part.value.strip_prefix(b"/") != Some(entry.as_bytes()) {
            return Some(());
        }
        let declared = element.try_get_attribute("ContentType").ok()??;
        if matched || declared.value.as_ref() != content_type {
            return None;
        }
        matched = true;
        Some(())
    });
    parsed.is_some() && matched
}

fn package_children(
    xml: &[u8],
    root: &[u8],
    namespace: &[u8],
    mut visit: impl FnMut(BytesStart<'_>) -> Option<()>,
) -> Option<()> {
    let mut reader = NsReader::from_reader(xml);
    reader.config_mut().trim_text(true);
    reader.config_mut().expand_empty_elements = true;
    let mut depth = 0;
    let mut root_closed = false;
    loop {
        let (resolved, event) = reader.read_resolved_event().ok()?;
        let in_package_namespace = matches!(
            resolved,
            ResolveResult::Bound(name) if name.as_ref() == namespace
        );
        match event {
            Event::Start(element) if depth == 0 => {
                if root_closed || !in_package_namespace || element.local_name().as_ref() != root {
                    return None;
                }
                depth = 1;
            }
            Event::Start(element) => {
                if depth == 1 && in_package_namespace {
                    visit(element)?;
                }
                depth += 1;
            }
            Event::End(_) if depth > 0 => {
                depth -= 1;
                root_closed = depth == 0;
            }
            Event::Decl(_) if depth == 0 && !root_closed => {}
            Event::Comment(_) | Event::PI(_) => {}
            Event::Eof => return root_closed.then_some(()),
            _ => return None,
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
        let bytes = office_fixture(
            "application/vnd.ms-word.document.macroEnabled.main+xml",
            &relationship("main/body.xml", "Internal"),
        );
        assert_eq!(verified("macro.docx", &bytes), OPAQUE);
        assert_eq!(verified("wrong.xlsx", &bytes), OPAQUE);
        assert_eq!(verified("archive.zip", &bytes), "application/zip");
    }

    #[test]
    fn office_media_requires_one_internal_relationship_to_the_declared_part() {
        for (file, content_type, verified_type) in [
            (
                "report.docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
                NativeFormat::Word.media_type(),
            ),
            (
                "report.xlsx",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
                NativeFormat::Spreadsheet.media_type(),
            ),
        ] {
            let path = "main/body.xml";
            let actual = relationship(path, "Internal");
            for (relationships, expected) in [
                (actual.clone(), verified_type),
                (relationship("/main/body.xml", "Internal"), verified_type),
                (relationship("missing/body.xml", "Internal"), OPAQUE),
                (relationship(path, "External"), OPAQUE),
                (
                    format!("{actual}{}", relationship("missing/body.xml", "Internal")),
                    OPAQUE,
                ),
                (String::new(), OPAQUE),
            ] {
                assert_eq!(
                    verified(file, &office_fixture(content_type, &relationships)),
                    expected
                );
            }
        }
    }

    fn relationship(target: &str, mode: &str) -> String {
        format!(
            r#"<Relationship Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="{target}" TargetMode="{mode}"/>"#
        )
    }

    #[test]
    fn office_metadata_requires_package_namespaces_and_direct_children() {
        let relation = relationship("main/body.xml", "Internal");
        for (file, content_type, expected) in [
            (
                "report.docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
                NativeFormat::Word.media_type(),
            ),
            (
                "report.xlsx",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
                NativeFormat::Spreadsheet.media_type(),
            ),
        ] {
            let declaration =
                format!(r#"<Override PartName="/main/body.xml" ContentType="{content_type}"/>"#);
            let relationships = package_relationships(&relation);
            let types = package_types(&declaration);
            assert_eq!(
                verified(file, &office_archive(&relationships, &types)),
                expected
            );
            let prefixed_relationships = format!(
                r#"<r:Relationships xmlns:r="{}">{}</r:Relationships>"#,
                std::str::from_utf8(RELATIONSHIPS_NAMESPACE).unwrap(),
                relation.replace("<Relationship ", "<r:Relationship "),
            );
            let prefixed_types = format!(
                r#"<ct:Types xmlns:ct="{}">{}</ct:Types>"#,
                std::str::from_utf8(CONTENT_TYPES_NAMESPACE).unwrap(),
                declaration.replace("<Override ", "<ct:Override "),
            );
            assert_eq!(
                verified(
                    file,
                    &office_archive(&prefixed_relationships, &prefixed_types)
                ),
                expected
            );
            for invalid in [
                format!(r#"<Relationships xmlns="urn:unrelated">{relation}</Relationships>"#),
                package_relationships(&format!("<Notes>{relation}</Notes>")),
                package_relationships(
                    &relation.replace("<Relationship ", r#"<Relationship xmlns="urn:unrelated" "#),
                ),
            ] {
                assert_eq!(
                    verified(file, &office_archive(&invalid, &types)),
                    OPAQUE,
                    "{invalid}"
                );
            }
            for invalid in [
                format!(r#"<Types xmlns="urn:unrelated">{declaration}</Types>"#),
                package_types(&format!("<Notes>{declaration}</Notes>")),
                package_types(
                    &declaration.replace("<Override ", r#"<Override xmlns="urn:unrelated" "#),
                ),
            ] {
                assert_eq!(
                    verified(file, &office_archive(&relationships, &invalid)),
                    OPAQUE,
                    "{invalid}"
                );
            }
        }
    }

    fn office_fixture(content_type: &str, relationships: &str) -> Vec<u8> {
        office_archive(
            &package_relationships(relationships),
            &package_types(&format!(
                r#"<Override PartName="/main/body.xml" ContentType="{content_type}"/>"#
            )),
        )
    }

    fn package_relationships(children: &str) -> String {
        format!(
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{children}</Relationships>"#
        )
    }

    fn package_types(children: &str) -> String {
        format!(
            r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">{children}</Types>"#
        )
    }

    fn office_archive(relationships: &str, manifest: &str) -> Vec<u8> {
        let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in [
            ("main/body.xml", b"<test/>".as_slice()),
            ("_rels/.rels", relationships.as_bytes()),
            ("[Content_Types].xml", manifest.as_bytes()),
        ] {
            archive
                .start_file(name, SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }
}

use super::*;
use std::io::{Cursor, Write};

pub(crate) fn fixtures() -> Vec<(Document, Vec<u8>)> {
    vec![
        (
            Document::Text,
            "A small document.\n世界\n".as_bytes().to_vec(),
        ),
        (Document::Pdf, b"%PDF-1.7\nfixture\n%%EOF".to_vec()),
        (Document::Word, compound(&word_streams())),
        (Document::Excel, compound(&excel_streams())),
        (Document::PowerPoint, compound(&powerpoint_streams())),
        (Document::WordXml, package(Document::WordXml)),
        (Document::ExcelXml, package(Document::ExcelXml)),
        (Document::PowerPointXml, package(Document::PowerPointXml)),
    ]
}

fn compound(streams: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut cfb = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    for (name, bytes) in streams {
        cfb.create_stream(name).unwrap().write_all(bytes).unwrap();
    }
    cfb.into_inner().into_inner()
}
fn word_streams() -> Vec<(&'static str, Vec<u8>)> {
    let mut word = vec![0; 68];
    word[..4].copy_from_slice(&[0xec, 0xa5, 0xc1, 0]);
    word[24..28].copy_from_slice(&64u32.to_le_bytes());
    word[28..32].copy_from_slice(&68u32.to_le_bytes());
    word[64..].copy_from_slice(b"Text");
    vec![("WordDocument", word), ("0Table", vec![0; 32])]
}
fn record(kind: u16, bytes: &[u8]) -> Vec<u8> {
    [
        kind.to_le_bytes().as_slice(),
        &(bytes.len() as u16).to_le_bytes(),
        bytes,
    ]
    .concat()
}
fn excel_streams() -> Vec<(&'static str, Vec<u8>)> {
    let mut bof = vec![0; 16];
    bof[..4].copy_from_slice(&[0, 6, 5, 0]);
    let body = [
        record(0x0809, &bof),
        record(0x0042, &[0xb0, 4]),
        record(0x000a, &[]),
    ]
    .concat();
    vec![("Workbook", body)]
}
fn powerpoint_streams() -> Vec<(&'static str, Vec<u8>)> {
    let mut document = vec![0x0f, 0, 0xe8, 3];
    document.extend(48u32.to_le_bytes());
    document.extend([0, 0, 0xe9, 3]);
    document.extend(40u32.to_le_bytes());
    document.extend([0; 40]);
    let mut user = vec![0, 0, 0xf6, 0x0f];
    user.extend(20u32.to_le_bytes());
    user.extend(20u32.to_le_bytes());
    user.extend(0xe391c05fu32.to_le_bytes());
    user.extend([0; 12]);
    vec![("PowerPoint Document", document), ("Current User", user)]
}
pub(crate) fn package(document: Document) -> Vec<u8> {
    zip(&parts(document), zip::CompressionMethod::Deflated)
}
fn parts(document: Document) -> Vec<(String, Vec<u8>)> {
    let (path, mime, root, ns) = match document {
        Document::WordXml => (
            "word/document.xml",
            "wordprocessingml.document",
            "document",
            "wordprocessingml",
        ),
        Document::ExcelXml => (
            "xl/workbook.xml",
            "spreadsheetml.sheet",
            "workbook",
            "spreadsheetml",
        ),
        Document::PowerPointXml => (
            "ppt/presentation.xml",
            "presentationml.presentation",
            "presentation",
            "presentationml",
        ),
        _ => unreachable!(),
    };
    vec![
        ("[Content_Types].xml".into(), format!(r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Override PartName="/{path}" ContentType="application/vnd.openxmlformats-officedocument.{mime}.main+xml"/></Types>"#).into_bytes()),
        ("_rels/.rels".into(), format!(r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="{path}"/></Relationships>"#).into_bytes()),
        (path.into(), format!(r#"<m:{root} xmlns:m="http://schemas.openxmlformats.org/{ns}/2006/main"></m:{root}>"#).into_bytes()),
    ]
}
fn zip(parts: &[(String, Vec<u8>)], method: zip::CompressionMethod) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, body) in parts {
        zip.start_file(
            name,
            zip::write::SimpleFileOptions::default().compression_method(method),
        )
        .unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap().into_inner()
}
fn replace(part: &mut (String, Vec<u8>), from: &str, to: &str) {
    part.1 = String::from_utf8(part.1.clone())
        .unwrap()
        .replace(from, to)
        .into_bytes();
}

#[test]
fn complete_document_bytes_determine_all_eight_types() {
    for (kind, bytes) in fixtures() {
        assert_eq!(inspect(&bytes), Some(kind), "{kind:?}");
        if kind != Document::Text {
            assert_eq!(
                inspect(&bytes[..bytes.len() / 2]),
                None,
                "truncated {kind:?}"
            );
        }
    }
}
#[test]
fn text_checks_the_entire_encoding_and_refuses_binary_controls() {
    for little in [true, false] {
        let mut bytes = if little {
            vec![0xff, 0xfe]
        } else {
            vec![0xfe, 0xff]
        };
        for unit in "Document 😀\n".encode_utf16() {
            bytes.extend(if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            });
        }
        assert_eq!(inspect(&bytes), Some(Document::Text));
        bytes.push(0);
        assert_eq!(inspect(&bytes), None);
    }
    for suffix in [
        vec![0],
        vec![0xff],
        vec![0xe2, 0x82],
        vec![0x1b],
        vec![0xc0, 0x80],
    ] {
        let bytes = [vec![b'a'; 65], suffix].concat();
        assert_eq!(inspect(&bytes), None);
    }
    assert_eq!(inspect(b"\xff\xfe\x00\xd8"), None); // unpaired UTF-16 surrogate
    assert_eq!(inspect(b"\xef\xbb\xbf\xff"), None);
    assert_eq!(inspect(b"\x89PNG\r\n\x1a\n"), None);
    assert_eq!(inspect(b"%PDF-1.7\ntruncated"), None);
}
#[test]
fn compound_identity_requires_valid_chains_and_matching_unencrypted_streams() {
    for defect in 0..6 {
        let mut streams = word_streams();
        match defect {
            0 => streams[0].1[0] = 0,
            1 => streams[0].1[11] = 1, // encrypted FIB
            2 => streams[0].1[11] = 2, // missing selected 1Table
            3 => streams[0].1[28..32].copy_from_slice(&u32::MAX.to_le_bytes()),
            4 => {
                streams.pop();
            }
            _ => streams.extend(excel_streams()),
        }
        assert_eq!(inspect(&compound(&streams)), None, "{defect}");
    }
    let mut xls = excel_streams();
    xls[0].1.splice(20..20, record(0x002f, &[1, 0]));
    assert_eq!(inspect(&compound(&xls)), None);
    let mut ppt = powerpoint_streams();
    ppt[1].1[12..16].copy_from_slice(&0xf3d1c4dfu32.to_le_bytes());
    assert_eq!(inspect(&compound(&ppt)), None);
    let mut cfb = compound(&word_streams());
    cfb[44..48].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(inspect(&cfb), None);
    assert_eq!(inspect(&compound(&[("Unrelated", vec![0; 20])])), None);
}
#[test]
fn package_identity_cannot_come_from_zip_names_labels_or_external_relationships() {
    for defect in 0..9 {
        let mut parts = parts(Document::WordXml);
        match defect {
            0 => {
                parts[2].1 = b"not XML".to_vec();
            }
            1 => replace(
                &mut parts[0],
                "wordprocessingml.document.main+xml",
                "wordprocessingml.template.main+xml",
            ),
            2 => replace(
                &mut parts[1],
                "Target=\"word/document.xml\"",
                "Target=\"https://files.example/document.xml\" TargetMode=\"External\"",
            ),
            3 => replace(
                &mut parts[2],
                "wordprocessingml/2006/main",
                "wrong/namespace",
            ),
            4 => replace(
                &mut parts[0],
                "<Types ",
                "<!DOCTYPE Types [<!ENTITY bomb 'expanded'>]><Types ",
            ),
            5 => parts.push(("../outside".into(), vec![1])),
            6 => parts.push(("Word/Document.xml".into(), vec![1])),
            7 => replace(&mut parts[2], "</m:document>", "</wrong>"),
            _ => replace(&mut parts[2], "</m:document>", "&unknown;</m:document>"),
        }
        assert_eq!(
            inspect(&zip(&parts, zip::CompressionMethod::Deflated)),
            None,
            "{defect}"
        );
    }
}
#[test]
fn packages_bound_expansion_and_entry_count_and_check_crc_of_unselected_parts() {
    let mut parts = parts(Document::ExcelXml);
    parts.push(("xl/oversize.bin".into(), vec![0; 8 * 1024 * 1024 + 1]));
    let compressed = zip(&parts, zip::CompressionMethod::Deflated);
    assert!(compressed.len() < 65536);
    assert_eq!(inspect(&compressed), None);
    parts.pop();
    parts.push(("xl/other.bin".into(), b"CRC_SENTINEL".to_vec()));
    let mut bytes = zip(&parts, zip::CompressionMethod::Stored);
    let at = bytes
        .windows(12)
        .position(|b| b == b"CRC_SENTINEL")
        .unwrap();
    bytes[at] ^= 1;
    assert_eq!(inspect(&bytes), None);
    let mut bytes = package(Document::WordXml);
    let end = bytes.len() - 22;
    bytes[end + 8..end + 12].copy_from_slice(&[0xff; 4]);
    assert_eq!(inspect(&bytes), None);
}

#[test]
fn package_xml_has_bounded_depth_and_no_invalid_numeric_entities() {
    for body in [
        "<x>".repeat(129) + &"</x>".repeat(129),
        "&#0;".into(),
        "<x a=\"&#0;\"/>".into(),
        "<!-- invalid -- comment -->".into(),
        format!(
            "<x {}/>",
            (0..65)
                .map(|i| format!("xmlns:p{i}=\"urn:{i}\" "))
                .collect::<String>()
        ),
        format!(
            "<x {}/>",
            (0..65).map(|i| format!("a{i}=\"v\" ")).collect::<String>()
        ),
    ] {
        let mut parts = parts(Document::WordXml);
        replace(&mut parts[2], "</m:document>", &(body + "</m:document>"));
        assert_eq!(
            inspect(&zip(&parts, zip::CompressionMethod::Deflated)),
            None
        );
    }
    let mut parts = parts(Document::WordXml);
    replace(&mut parts[2], "</m:document>", "&#65;&amp;</m:document>");
    assert_eq!(
        inspect(&zip(&parts, zip::CompressionMethod::Stored)),
        Some(Document::WordXml)
    );
}

#[test]
fn package_local_names_must_agree_with_central_names() {
    let mut bytes = package(Document::WordXml);
    assert_eq!(&bytes[30..49], b"[Content_Types].xml");
    bytes[30] = b'X';
    assert_eq!(inspect(&bytes), None);
}

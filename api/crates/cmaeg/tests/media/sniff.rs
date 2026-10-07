//! Magic-byte classification, and the declared text hint used before a fetch.
use cmaeg::media::sniff::{sniff, text_like_hint, Family, Sniffed};

fn with_tail(head: &[u8]) -> Vec<u8> {
    let mut bytes = head.to_vec();
    bytes.extend_from_slice(&[0; 32]);
    bytes
}

#[test]
fn media_magic_bytes_keep_their_family() {
    for (name, bytes, family, mime) in [
        (
            "png",
            with_tail(b"\x89PNG\r\n\x1a\n"),
            Family::Image,
            "image/png",
        ),
        (
            "jpeg",
            with_tail(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Family::Image,
            "image/jpeg",
        ),
        ("gif", with_tail(b"GIF89a"), Family::Image, "image/gif"),
        (
            "webp",
            with_tail(b"RIFF\x24\0\0\0WEBPVP8 "),
            Family::Image,
            "image/webp",
        ),
        ("mp3", with_tail(b"ID3\x03\0"), Family::Audio, "audio/mpeg"),
        ("ogg", with_tail(b"OggS\0\x02"), Family::Audio, "audio/ogg"),
        (
            "mp4",
            with_tail(b"\0\0\0\x18ftypisom\0\0\x02\0"),
            Family::Video,
            "video/mp4",
        ),
        (
            "webm",
            with_tail(&[0x1A, 0x45, 0xDF, 0xA3]),
            Family::Video,
            "video/webm",
        ),
    ] {
        assert_eq!(sniff(&bytes), Sniffed { family, mime }, "{name}");
    }
}

#[test]
fn documents_and_archives_are_other_with_their_mime() {
    assert_eq!(
        sniff(&with_tail(b"%PDF-1.7\n")),
        Sniffed {
            family: Family::Other,
            mime: "application/pdf"
        }
    );
    assert_eq!(
        sniff(&with_tail(b"PK\x03\x04\x14\0")),
        Sniffed {
            family: Family::Other,
            mime: "application/zip"
        }
    );
}

#[test]
fn text_is_utf8_without_nul_or_marked_by_a_byte_order_mark() {
    let text = Sniffed {
        family: Family::Text,
        mime: "text/plain",
    };
    assert_eq!(sniff("héllo, wörld\n".as_bytes()), text);
    assert_eq!(sniff(b"\xEF\xBB\xBFwith bom"), text);
    assert_eq!(sniff(&[0xFF, 0xFE, b'h', 0, b'i', 0]), text);
    assert_eq!(sniff(&[0xFE, 0xFF, 0, b'h', 0, b'i']), text);
    // A head cut inside a multibyte character is still text.
    let euro = "price: 5€".as_bytes();
    assert_eq!(sniff(&euro[..euro.len() - 1]), text);
}

#[test]
fn binary_with_a_nul_byte_is_octet_stream() {
    let other = Sniffed {
        family: Family::Other,
        mime: "application/octet-stream",
    };
    assert_eq!(sniff(&[b'a', b'b', 0, b'c']), other);
    assert_eq!(sniff(&[0x01, 0x02, 0x03, 0xC3, b'(']), other);
}

#[test]
fn the_sniffed_type_wins_over_any_declaration() {
    // Nothing declared is consulted: a PNG is a PNG whatever it was called.
    assert!(text_like_hint(Some("text/plain"), Some("notes.txt")));
    assert_eq!(
        sniff(&with_tail(b"\x89PNG\r\n\x1a\n")).family,
        Family::Image
    );
}

#[test]
fn text_like_hints_cover_declared_text_types_and_extensions() {
    for mime in [
        "text/plain",
        "TEXT/CSV",
        "text/markdown; charset=utf-8",
        "application/json",
        "application/xml",
        "application/yaml",
        "application/x-yaml",
        "application/javascript",
        "application/x-sh",
    ] {
        assert!(text_like_hint(Some(mime), None), "{mime}");
    }
    for name in [
        "a.txt",
        "README.MD",
        "notes.markdown",
        "data.csv",
        "x.tsv",
        "x.json",
        "x.jsonl",
        "x.yaml",
        "x.yml",
        "x.xml",
        "x.html",
        "x.htm",
        "x.log",
        "x.ini",
        "x.toml",
        "x.conf",
        "x.cfg",
        "x.py",
        "x.rs",
        "x.ts",
        "x.js",
        "x.go",
        "x.java",
        "x.c",
        "x.h",
        "x.cpp",
        "x.sh",
        "x.sql",
        "x.rb",
        "x.kt",
        "x.swift",
        "x.php",
        "archive.tar.Json",
    ] {
        assert!(text_like_hint(None, Some(name)), "{name}");
    }
    for (mime, name) in [
        (None, None),
        (Some("image/png"), Some("photo.png")),
        (Some("application/pdf"), Some("report.pdf")),
        (Some("application/octet-stream"), Some("txt")),
        (None, Some("Makefile")),
        (None, Some("x.jsx")),
    ] {
        assert!(!text_like_hint(mime, name), "{mime:?} {name:?}");
    }
}

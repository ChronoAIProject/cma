//! Text decoding, sanitizing, paging and excerpts.
use bytes::Bytes;
use cmaeg::media::{
    body::FetchedBody,
    cache::Weighted,
    text::{
        decode, excerpt, normalize, TextDocument, TextError, EXCERPT_BYTES, EXCERPT_READ_LIMIT,
        PAGE_BYTES, TEXT_READ_LIMIT,
    },
};

fn body(bytes: impl Into<Vec<u8>>, truncated: bool) -> FetchedBody {
    FetchedBody {
        bytes: Bytes::from(bytes.into()),
        truncated,
    }
}

fn text_of(doc: &TextDocument) -> String {
    doc.pages.concat()
}

#[test]
fn the_limits_are_as_documented() {
    assert_eq!(TEXT_READ_LIMIT, 1_048_576);
    assert_eq!(EXCERPT_READ_LIMIT, 262_144);
    assert_eq!(PAGE_BYTES, 16_384);
    assert_eq!(EXCERPT_BYTES, 4_096);
}

#[test]
fn utf8_with_and_without_a_bom_decodes() {
    for input in [
        "héllo wörld\n".as_bytes().to_vec(),
        [b"\xEF\xBB\xBF".as_slice(), "héllo wörld\n".as_bytes()].concat(),
    ] {
        let doc = decode(&body(input, false)).unwrap();
        assert_eq!(doc.pages, ["héllo wörld\n"]);
        assert!(!doc.truncated);
        assert_eq!(doc.weight(), "héllo wörld\n".len() as u64);
    }
}

#[test]
fn utf16_in_either_byte_order_decodes() {
    let text = "héllo 𝄞 wörld";
    let units: Vec<u16> = text.encode_utf16().collect();
    let little: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain(units.iter().flat_map(|unit| unit.to_le_bytes()))
        .collect();
    let big: Vec<u8> = [0xFE, 0xFF]
        .into_iter()
        .chain(units.iter().flat_map(|unit| unit.to_be_bytes()))
        .collect();
    for input in [little, big] {
        assert_eq!(text_of(&decode(&body(input, false)).unwrap()), text);
    }
    // An unpaired surrogate becomes U+FFFD instead of failing.
    let unpaired = [0xFF, 0xFE, 0x00, 0xD8, b'a', 0x00];
    assert_eq!(
        text_of(&decode(&body(unpaired.to_vec(), false)).unwrap()),
        "\u{FFFD}a"
    );
    // A body cut between the halves of a pair loses only the cut half.
    let cut = [0xFF, 0xFE, b'a', 0x00, 0x34, 0xD8, 0x1E];
    assert_eq!(text_of(&decode(&body(cut.to_vec(), true)).unwrap()), "a");
}

#[test]
fn binary_is_not_text() {
    let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    assert_eq!(decode(&body(png, false)).unwrap_err(), TextError::NotText);
    let invalid = vec![b'a', 0xC3, b'b'];
    assert_eq!(
        decode(&body(invalid, true)).unwrap_err(),
        TextError::NotText
    );
}

#[test]
fn an_incomplete_multibyte_tail_is_accepted_only_when_truncated() {
    let mut input = "abc".as_bytes().to_vec();
    input.extend_from_slice(&"€".as_bytes()[..2]);
    assert_eq!(
        decode(&body(input.clone(), false)).unwrap_err(),
        TextError::NotText
    );
    let doc = decode(&body(input, true)).unwrap();
    assert_eq!(doc.pages, ["abc"]);
    assert!(doc.truncated);
}

#[test]
fn forty_kib_gives_three_pages_on_char_boundaries() {
    // Three-byte characters never align with the 16 KiB page size.
    let text = "€".repeat(40 * 1024 / 3);
    let doc = decode(&body(text.clone(), false)).unwrap();
    assert_eq!(doc.pages.len(), 3);
    assert!(doc.pages.iter().all(|page| page.len() <= PAGE_BYTES));
    assert!(doc.pages[..2]
        .iter()
        .all(|page| page.len() > PAGE_BYTES - 3));
    assert_eq!(text_of(&doc), text);
    // Empty text is one empty page.
    assert_eq!(decode(&body(Vec::new(), false)).unwrap().pages, [""]);
}

#[test]
fn excerpts_stop_on_a_char_boundary_and_report_more() {
    let doc = decode(&body("€".repeat(2_000), false)).unwrap();
    let (short, more) = excerpt(&doc, EXCERPT_BYTES);
    assert!(more);
    assert!(short.len() <= EXCERPT_BYTES && short.len() > EXCERPT_BYTES - 3);
    assert!(short.chars().all(|character| character == '€'));
    // Everything fits: nothing more, unless the body itself was cut.
    let small = decode(&body("short", false)).unwrap();
    assert_eq!(excerpt(&small, EXCERPT_BYTES), ("short".into(), false));
    let cut = decode(&body("short", true)).unwrap();
    assert_eq!(excerpt(&cut, EXCERPT_BYTES), ("short".into(), true));
    // An excerpt spanning pages joins them.
    let long = decode(&body("x".repeat(PAGE_BYTES + 10), false)).unwrap();
    let (joined, more) = excerpt(&long, PAGE_BYTES + 5);
    assert_eq!((joined.len(), more), (PAGE_BYTES + 5, true));
}

#[test]
fn controls_and_bidi_overrides_are_stripped_and_line_endings_normalized() {
    let input = "a\r\nb\rc\n\td\u{0}e\u{7}f\u{1B}[31mg\u{7F}h\u{85}i\u{9F}j\u{202E}k\u{2066}l\u{200F}m\u{200E}n";
    let expected = "a\nb\nc\n\tdef[31mghijklmn";
    assert_eq!(normalize(input), expected);
    assert_eq!(text_of(&decode(&body(input, false)).unwrap()), expected);
    // Ordinary non-ASCII text is untouched.
    assert_eq!(normalize("日本語 עברית ✓"), "日本語 עברית ✓");
}

#[test]
fn debug_shows_sizes_not_content() {
    let doc = decode(&body("a secret line", false)).unwrap();
    let shown = format!("{doc:?}");
    assert!(!shown.contains("secret"), "{shown}");
    assert!(shown.contains("bytes: 13"), "{shown}");
}

//! Sender-declared facts always normalize into the domain the validator accepts.
use cmaeg_core::{
    activity::{
        validate_attachments, Attachment, AttachmentKind, AttachmentSource, FactRepairs,
        MediaVariant, SourceKind, MAX_DIMENSION, MAX_DURATION_SECONDS, MAX_EMOJI_BYTES,
        MAX_FILENAME_BYTES, MAX_NYXID_INDEX,
    },
    channel::Platform,
};

const BASE: &str = "/kind/attachments";

fn bare() -> Attachment {
    Attachment {
        reference: "fixture:item-0".into(),
        kind: AttachmentKind::File,
        filename: None,
        mime_type: None,
        size_bytes: None,
        source: None,
        variant: None,
        duration_seconds: None,
        width: None,
        height: None,
        thumbnail: None,
        emoji: None,
    }
}

fn normalized(edit: impl FnOnce(&mut Attachment)) -> (Attachment, Vec<&'static str>) {
    let mut attachment = bare();
    edit(&mut attachment);
    let repairs = attachment.normalize_facts();
    validate_attachments(std::slice::from_ref(&attachment), &SourceKind::Custom, BASE).unwrap();
    (attachment, repairs.fields().collect())
}

#[test]
fn an_oversized_name_keeps_its_extension_and_whole_characters() {
    let stem = "报".repeat(99);
    let (attachment, repairs) = normalized(|a| a.filename = Some(format!("{stem}.pdf")));
    let name = attachment.filename.unwrap();
    assert!(name.len() <= MAX_FILENAME_BYTES, "{}", name.len());
    assert!(name.ends_with("….pdf"));
    let kept = name.strip_suffix("….pdf").unwrap();
    assert!(kept.chars().all(|c| c == '报') && stem.starts_with(kept));
    assert_eq!(kept.len(), (MAX_FILENAME_BYTES - "….pdf".len()) / 3 * 3);
    assert_eq!(repairs, ["filename"]);
}

#[test]
fn an_oversized_name_without_an_extension_is_cut_with_an_ellipsis() {
    let original = "飞书文档-季度经营分析报告_最终版".repeat(9);
    assert!(original.len() >= 400);
    let (attachment, repairs) = normalized(|a| a.filename = Some(original.clone()));
    let name = attachment.filename.unwrap();
    assert!(name.len() <= MAX_FILENAME_BYTES && name.ends_with('…'));
    assert!(original.starts_with(name.strip_suffix('…').unwrap()));
    assert_eq!(repairs, ["filename"]);
    let long_extension = format!("{}.{}", "x".repeat(300), "e".repeat(17));
    let (attachment, _) = normalized(|a| a.filename = Some(long_extension));
    assert!(attachment.filename.unwrap().ends_with("x…"));
}

#[test]
fn names_lose_controls_and_bidirectional_overrides() {
    let (attachment, repairs) =
        normalized(|a| a.filename = Some(" invoice\u{202E}fdp\u{0007}.exe ".into()));
    assert_eq!(attachment.filename.as_deref(), Some("invoicefdp.exe"));
    assert_eq!(repairs, ["filename"]);
    let (attachment, repairs) =
        normalized(|a| a.filename = Some("\u{0000}\u{202A}\u{0085}\u{2069}".into()));
    assert_eq!(attachment.filename, None);
    assert_eq!(repairs, ["filename"]);
    let (attachment, _) = normalized(|a| a.filename = Some("   ".into()));
    assert_eq!(attachment.filename, None);
}

#[test]
fn mime_types_keep_only_a_lowercase_essence() {
    for (declared, expected) in [
        ("text/plain; charset=utf-8", Some("text/plain")),
        ("Image/JPEG", Some("image/jpeg")),
        ("  Audio/OGG ; codecs=opus ", Some("audio/ogg")),
        ("not a mime", None),
        ("image/", None),
        (";", None),
    ] {
        let (attachment, repairs) = normalized(|a| a.mime_type = Some(declared.into()));
        assert_eq!(attachment.mime_type.as_deref(), expected, "{declared}");
        assert_eq!(repairs, ["mime_type"], "{declared}");
    }
}

#[test]
fn dimensions_survive_only_as_a_valid_pair() {
    for (width, height) in [
        (Some(0), Some(0)),
        (Some(640), None),
        (None, Some(480)),
        (Some(640), Some(0)),
        (Some(MAX_DIMENSION + 1), Some(480)),
        (Some(u32::MAX), Some(u32::MAX)),
    ] {
        let (attachment, repairs) = normalized(|a| (a.width, a.height) = (width, height));
        assert_eq!((attachment.width, attachment.height), (None, None));
        assert_eq!(repairs, ["dimensions"]);
    }
    let (attachment, repairs) =
        normalized(|a| (a.width, a.height) = (Some(1), Some(MAX_DIMENSION)));
    assert_eq!(
        (attachment.width, attachment.height),
        (Some(1), Some(MAX_DIMENSION))
    );
    assert!(repairs.is_empty());
}

#[test]
fn an_impossible_duration_is_dropped_and_an_oversized_emoji_removed() {
    let (attachment, repairs) = normalized(|a| a.duration_seconds = Some(100_000));
    assert_eq!(attachment.duration_seconds, None);
    assert_eq!(repairs, ["duration_seconds"]);
    let (attachment, repairs) = normalized(|a| a.duration_seconds = Some(MAX_DURATION_SECONDS));
    assert_eq!(attachment.duration_seconds, Some(MAX_DURATION_SECONDS));
    assert!(repairs.is_empty());
    let (attachment, repairs) = normalized(|a| a.emoji = Some("👍".repeat(10)));
    assert_eq!(attachment.emoji, None);
    assert_eq!(repairs, ["emoji"]);
    let (attachment, repairs) = normalized(|a| a.emoji = Some("\u{202E}👍".into()));
    assert_eq!(attachment.emoji.as_deref(), Some("👍"));
    assert_eq!(repairs, ["emoji"]);
}

#[test]
fn repairs_are_reported_exactly_and_never_for_valid_facts() {
    let mut valid = bare();
    valid.filename = Some("report.pdf".into());
    valid.mime_type = Some("application/pdf".into());
    valid.size_bytes = Some(u64::MAX);
    valid.variant = Some(MediaVariant::Document);
    valid.duration_seconds = Some(1);
    (valid.width, valid.height) = (Some(1), Some(1));
    valid.emoji = Some("📄".into());
    valid.source = Some(AttachmentSource::Url {
        url: "https://files.example/report.pdf".into(),
    });
    let before = valid.clone();
    assert_eq!(valid.normalize_facts(), FactRepairs::default());
    assert_eq!(valid, before);

    let mut broken = before.clone();
    broken.mime_type = Some("Application/PDF".into());
    broken.emoji = Some("x".repeat(MAX_EMOJI_BYTES + 1));
    let repairs = broken.normalize_facts();
    assert!(repairs.contains(FactRepairs::MIME_TYPE) && repairs.contains(FactRepairs::EMOJI));
    assert!(!repairs.contains(FactRepairs::FILENAME));
    assert_eq!(repairs.fields().collect::<Vec<_>>(), ["mime_type", "emoji"]);
    assert_eq!(
        (broken.reference, broken.kind),
        (before.reference, before.kind)
    );
    assert_eq!(
        (broken.source, broken.size_bytes),
        (before.source, before.size_bytes)
    );
    assert_eq!(broken.variant, before.variant);
}

#[test]
fn minimal_keeps_only_the_identity_and_locator() {
    let mut full = bare();
    full.filename = Some("\u{202E}".into());
    full.mime_type = Some("bad".into());
    full.width = Some(0);
    full.thumbnail = Some(AttachmentSource::Url {
        url: "http://bad".into(),
    });
    full.source = Some(AttachmentSource::NyxidRelay { index: 3 });
    let minimal = full.minimal();
    let mut expected = bare();
    expected.source = Some(AttachmentSource::NyxidRelay { index: 3 });
    assert_eq!(minimal, expected);
}

/// Deterministic generator so a failing case reproduces exactly.
struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn pick<T: Copy>(&mut self, values: &[T]) -> T {
        values[self.below(values.len() as u64) as usize]
    }

    fn char(&mut self) -> char {
        let (start, end) = self.pick(&[
            (0x20, 0x7E),       // printable ASCII
            (0x30, 0x39),       // digits
            (0x00, 0x1F),       // C0 controls
            (0x7F, 0x9F),       // DEL and C1 controls
            (0x200E, 0x200F),   // bidi marks
            (0x202A, 0x202E),   // bidi embeddings and overrides
            (0x2066, 0x2069),   // bidi isolates
            (0x4E00, 0x9FFF),   // CJK
            (0x1F300, 0x1F64F), // emoji
            (0x3000, 0x3000),   // ideographic space
            (0x2E, 0x2F),       // dot and slash
            (0x3B, 0x3B),       // semicolon
        ]);
        char::from_u32(start + self.below(u64::from(end - start + 1)) as u32).unwrap()
    }

    fn text(&mut self, max_chars: u64) -> String {
        let length = self.below(max_chars + 1);
        let mut text: String = (0..length).map(|_| self.char()).collect();
        if self.below(3) == 0 {
            text.push('.');
            let extension = self.below(20) + 1;
            text.extend((0..extension).map(|_| self.pick(&['p', 'D', 'f', '4', 'é'])));
        }
        text
    }

    fn mime(&mut self) -> String {
        let parts = ["image", "IMAGE", "Video", "application", "x-", "-bad", ""];
        let subtypes = ["jpeg", "PNG", "vnd.ms-excel", "ogg", "a b", "!", ""];
        match self.below(4) {
            0 => self.text(40),
            _ => format!(
                "{}{}/{}{}{}",
                self.pick(&["", " ", "\t"]),
                self.pick(&parts),
                self.pick(&subtypes),
                self.pick(&["", "; charset=UTF-8", ";", " ; q=1", "/x"]),
                self.pick(&["", " ", "\u{0007}"]),
            ),
        }
    }

    fn number(&mut self) -> Option<u32> {
        match self.below(9) {
            0 => None,
            1 => Some(0),
            2 => Some(u32::MAX),
            3 => Some(MAX_DIMENSION),
            4 => Some(MAX_DIMENSION + 1),
            5 => Some(MAX_DURATION_SECONDS),
            6 => Some(MAX_DURATION_SECONDS + 1),
            7 => Some(self.below(1_000) as u32),
            _ => Some(self.next() as u32),
        }
    }

    fn maybe<T>(&mut self, value: impl FnOnce(&mut Self) -> T) -> Option<T> {
        (self.below(4) != 0).then(|| value(self))
    }

    fn telegram_id(&mut self, max: u64) -> String {
        let length = self.below(max) + 1;
        let alphabet = b"ABCxyz0189_-";
        (0..length)
            .map(|_| char::from(alphabet[self.below(alphabet.len() as u64) as usize]))
            .collect()
    }

    /// A valid locator (or none) and thumbnail for a source chosen at random.
    fn located(
        &mut self,
    ) -> (
        SourceKind,
        Option<AttachmentSource>,
        Option<AttachmentSource>,
    ) {
        match self.below(3) {
            0 => {
                let index = self.below(u64::from(MAX_NYXID_INDEX) + 1) as u8;
                let source = SourceKind::NyxidRelay {
                    platform: Platform::Telegram,
                    route_id: "route-1".into(),
                };
                (
                    source,
                    self.maybe(|_| AttachmentSource::NyxidRelay { index }),
                    None,
                )
            }
            1 => {
                let source = SourceKind::Telegram {
                    platform: Platform::Telegram,
                    bot_id: "123456".into(),
                };
                let locator = |rng: &mut Self| AttachmentSource::Telegram {
                    file_id: rng.telegram_id(256),
                    file_unique_id: rng.telegram_id(128),
                };
                (source, self.maybe(locator), self.maybe(locator))
            }
            _ => {
                let locator = self.maybe(|rng| match rng.below(2) {
                    0 => AttachmentSource::Url {
                        url: format!("https://files.example/{}", rng.telegram_id(64)),
                    },
                    _ => AttachmentSource::Inline {
                        pointer: format!("/attachments/{}/url", rng.below(16)),
                    },
                });
                (SourceKind::Custom, locator, None)
            }
        }
    }
}

#[test]
fn every_random_fact_normalizes_into_the_validated_domain() {
    let variants = [
        MediaVariant::Photo,
        MediaVariant::Voice,
        MediaVariant::VideoNote,
        MediaVariant::Sticker,
        MediaVariant::Embedded,
    ];
    let mut rng = XorShift64(0x9E37_79B9_7F4A_7C15);
    for case in 0..10_000 {
        let (source, locator, thumbnail) = rng.located();
        let original = Attachment {
            reference: format!("fixture:item-{case}"),
            kind: rng.pick(&[
                AttachmentKind::Image,
                AttachmentKind::Audio,
                AttachmentKind::Other,
            ]),
            filename: rng.maybe(|rng| rng.text(400)),
            mime_type: rng.maybe(XorShift64::mime),
            size_bytes: rng.maybe(XorShift64::next),
            source: locator,
            variant: rng.maybe(|rng| rng.pick(&variants)),
            duration_seconds: rng.number(),
            width: rng.number(),
            height: rng.number(),
            thumbnail,
            emoji: rng.maybe(|rng| rng.text(20)),
        };
        let validate = |attachment: &Attachment| {
            validate_attachments(std::slice::from_ref(attachment), &source, BASE)
        };
        validate(&original.minimal())
            .unwrap_or_else(|error| panic!("case {case}: minimal refused: {error}"));

        let mut attachment = original.clone();
        let repairs = attachment.normalize_facts();
        validate(&attachment).unwrap_or_else(|error| panic!("case {case}: {error}"));
        for (flag, changed) in [
            (
                FactRepairs::FILENAME,
                attachment.filename != original.filename,
            ),
            (
                FactRepairs::MIME_TYPE,
                attachment.mime_type != original.mime_type,
            ),
            (
                FactRepairs::DIMENSIONS,
                (attachment.width, attachment.height) != (original.width, original.height),
            ),
            (
                FactRepairs::DURATION,
                attachment.duration_seconds != original.duration_seconds,
            ),
            (FactRepairs::EMOJI, attachment.emoji != original.emoji),
        ] {
            assert_eq!(repairs.contains(flag), changed, "case {case}: {flag:?}");
        }
        assert_eq!(attachment.minimal(), original.minimal(), "case {case}");
        assert_eq!(
            (
                attachment.variant,
                attachment.size_bytes,
                &attachment.thumbnail
            ),
            (original.variant, original.size_bytes, &original.thumbnail),
            "case {case}"
        );
        let mut again = attachment.clone();
        assert!(
            again.normalize_facts().is_empty(),
            "case {case}: not idempotent"
        );
        assert_eq!(again, attachment, "case {case}");
    }
}

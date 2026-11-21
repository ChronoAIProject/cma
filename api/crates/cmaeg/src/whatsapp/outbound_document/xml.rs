//! UTF-8 package XML with bounded nesting/events; DTDs and custom entities refused.
use quick_xml::{events::Event, name::ResolveResult, reader::NsReader, XmlVersion};
use std::collections::BTreeMap;

pub(super) type Attributes = BTreeMap<String, String>;
fn valid_char(c: char) -> bool {
    matches!(c as u32, 9 | 10 | 13 | 0x20..=0xd7ff | 0xe000..=0xfffd | 0x10000..=0x10ffff)
}

pub(super) fn parse(
    bytes: &[u8],
    root: &str,
    namespaces: &[&str],
    mut visit: impl FnMut(usize, &str, &str, &Attributes) -> Option<()>,
) -> Option<()> {
    if !std::str::from_utf8(bytes).ok()?.chars().all(valid_char) {
        return None;
    }
    let mut reader = NsReader::from_reader(bytes);
    reader.config_mut().check_comments = true;
    reader.resolver_mut().set_max_declarations_per_element(64);
    let mut depth = 0usize;
    let mut seen = false;
    let mut closed = false;
    let mut declaration = false;
    for _ in 0..131072 {
        let (namespace, event) = reader.read_resolved_event().ok()?;
        match event {
            Event::Start(ref node) | Event::Empty(ref node) => {
                if closed || depth >= 128 || node.name().as_ref().len() > 256 {
                    return None;
                }
                let namespace = match &namespace {
                    ResolveResult::Bound(n) => std::str::from_utf8(n.as_ref()).ok()?,
                    ResolveResult::Unbound => "",
                    ResolveResult::Unknown(_) => return None,
                };
                let local = node.local_name();
                let local = std::str::from_utf8(local.as_ref()).ok()?;
                if depth == 0 {
                    if seen || local != root || !namespaces.contains(&namespace) {
                        return None;
                    }
                    seen = true;
                }
                let mut attrs = Attributes::new();
                for attribute in node.attributes().take(65) {
                    let attribute = attribute.ok()?;
                    if attribute.key.as_ref().len() > 256 || attribute.value.len() > 8192 {
                        return None;
                    }
                    let value = attribute.normalized_value(XmlVersion::Implicit1_0).ok()?;
                    if !value.chars().all(valid_char) {
                        return None;
                    }
                    attrs.insert(
                        std::str::from_utf8(attribute.key.as_ref()).ok()?.to_owned(),
                        value.into_owned(),
                    );
                }
                if attrs.len() > 64 {
                    return None;
                }
                visit(depth, namespace, local, &attrs)?;
                if matches!(event, Event::Start(_)) {
                    depth += 1;
                } else if depth == 0 {
                    closed = true;
                }
            }
            Event::End(_) => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    closed = true;
                }
            }
            Event::DocType(_) => return None,
            Event::Text(t) if depth == 0 => {
                if !t.iter().all(u8::is_ascii_whitespace) {
                    return None;
                }
            }
            Event::CData(_) if depth == 0 => return None,
            Event::GeneralRef(r) => {
                let entity: &[u8] = &r;
                let character = r.resolve_char_ref().ok()?;
                if depth == 0
                    || character.is_some_and(|c| !valid_char(c))
                    || (character.is_none()
                        && !matches!(entity, b"amp" | b"lt" | b"gt" | b"apos" | b"quot"))
                {
                    return None;
                }
            }
            Event::Decl(d) => {
                if seen
                    || declaration
                    || d.version().ok()?.as_ref() != b"1.0"
                    || d.encoding()
                        .is_some_and(|e| !e.is_ok_and(|e| e.eq_ignore_ascii_case(b"utf-8")))
                {
                    return None;
                }
                declaration = true;
            }
            Event::Eof => return (seen && closed && depth == 0).then_some(()),
            _ => {}
        }
    }
    None
}

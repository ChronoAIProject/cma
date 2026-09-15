use super::error::Error;
use layer2_core::sandbox::AgentEventStreamHeader;
use sha2::{Digest, Sha256};

pub(super) fn incarnation(header: &AgentEventStreamHeader) -> String {
    let bytes = serde_json::to_vec(&("cma.agent.cursor.v1", &header.id, &header.created_at))
        .expect("typed cursor identity serializes");
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn event(incarnation: &str, sequence: u64) -> String {
    format!("ag1.{incarnation}.{sequence}")
}
pub(super) fn page(incarnation: &str, through: u64, after: u64) -> String {
    format!("ags1.{incarnation}.{through}.{after}")
}
fn number(value: &str) -> Result<u64, Error> {
    let number = value.parse::<u64>().map_err(|_| Error::Cursor)?;
    if number.to_string() != value {
        return Err(Error::Cursor);
    }
    Ok(number)
}
fn parts<'a>(value: &'a str, prefix: &str, count: usize) -> Result<Vec<&'a str>, Error> {
    if value.len() > 128 {
        return Err(Error::Cursor);
    }
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != count
        || parts[0] != prefix
        || parts[1].len() != 64
        || !parts[1]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(Error::Cursor);
    }
    Ok(parts)
}
pub(super) fn parse_event(value: &str, identity: &str, head: u64) -> Result<u64, Error> {
    let parts = parts(value, "ag1", 3)?;
    let sequence = number(parts[2])?;
    if parts[1] != identity {
        return Err(Error::Incarnation);
    }
    if sequence > head {
        return Err(Error::BeyondHead);
    }
    Ok(sequence)
}
pub(super) fn parse_page(value: &str, identity: &str, head: u64) -> Result<(u64, u64), Error> {
    let parts = parts(value, "ags1", 4)?;
    let through = number(parts[2])?;
    let after = number(parts[3])?;
    if parts[1] != identity {
        return Err(Error::Incarnation);
    }
    if after >= through {
        return Err(Error::Cursor);
    }
    if through > head {
        return Err(Error::BeyondHead);
    }
    Ok((through, after))
}

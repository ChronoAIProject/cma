use serde::Serialize;

pub(crate) const WIRE_VERSION: u32 = 1;
pub(crate) const CONVERSATION: u8 = 0x02;
pub(crate) const CONVERSATION_GAP: u8 = 0x03;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Ready<'a> {
    pub t: &'static str,
    pub wire_version: u32,
    pub lanes: [&'static str; 2],
    pub agent_id: &'a str,
}

impl<'a> Ready<'a> {
    pub(crate) fn agent(agent_id: &'a str) -> Self {
        Self {
            t: "ready",
            wire_version: WIRE_VERSION,
            lanes: ["ctl", "conv"],
            agent_id,
        }
    }
}

pub(crate) fn frame(lane: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 1);
    frame.push(lane);
    frame.extend_from_slice(payload);
    frame
}

pub(crate) fn unframe(frame: &[u8]) -> Option<(u8, &[u8])> {
    frame.split_first().map(|(lane, payload)| (*lane, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_conversation_lanes_remain_stable() {
        assert_eq!(CONVERSATION, 0x02);
        assert_eq!(CONVERSATION_GAP, 0x03);
        assert_eq!(
            unframe(&frame(CONVERSATION, b"opaque")),
            Some((0x02, &b"opaque"[..]))
        );
    }
}

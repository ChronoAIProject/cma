use serde::Serialize;

pub(crate) const WIRE_VERSION: u32 = 1;
pub(crate) const CONVERSATION: u8 = 0x02;
pub(crate) const CONVERSATION_GAP: u8 = 0x03;
pub(crate) const ACKNOWLEDGED_CONVERSATION: u8 = 0x04;
pub(crate) const OUTPUT_ACKNOWLEDGEMENT: u8 = 0x05;
pub(crate) const OUTPUT_IDENTITY_BYTES: usize = 24;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Ready<'a> {
    pub t: &'static str,
    pub wire_version: u32,
    pub lanes: [&'static str; 2],
    pub agent_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_acknowledgement: Option<OutputAcknowledgement>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manager_ownership: Option<layer3_adapter::owner_protocol::OwnerAcknowledgement>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutputAcknowledgement {
    version: u32,
    stream_id: uuid::Uuid,
}

impl<'a> Ready<'a> {
    pub(crate) fn agent(agent_id: &'a str) -> Self {
        Self {
            t: "ready",
            wire_version: WIRE_VERSION,
            lanes: ["ctl", "conv"],
            agent_id,
            output_acknowledgement: None,
            manager_ownership: None,
        }
    }

    pub(crate) fn with_acknowledgement(mut self, stream_id: uuid::Uuid) -> Self {
        self.output_acknowledgement = Some(OutputAcknowledgement {
            version: 1,
            stream_id,
        });
        self
    }

    pub(crate) fn with_owner(
        mut self,
        owner: Option<layer3_adapter::owner_protocol::TransportOwner>,
    ) -> Self {
        self.manager_ownership =
            owner.map(
                |owner| layer3_adapter::owner_protocol::OwnerAcknowledgement {
                    owner,
                    protocol: layer3_adapter::owner_protocol::OWNER_PROTOCOL_VERSION,
                },
            );
        self
    }
}

pub(crate) fn acknowledged_frame(stream_id: uuid::Uuid, cursor: u64, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(1 + OUTPUT_IDENTITY_BYTES + payload.len());
    frame.push(ACKNOWLEDGED_CONVERSATION);
    frame.extend_from_slice(stream_id.as_bytes());
    frame.extend_from_slice(&cursor.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

pub(crate) fn output_acknowledgement(payload: &[u8]) -> Option<(uuid::Uuid, u64)> {
    if payload.len() != OUTPUT_IDENTITY_BYTES {
        return None;
    }
    let stream_id = uuid::Uuid::from_slice(&payload[..16]).ok()?;
    let cursor = u64::from_be_bytes(payload[16..].try_into().ok()?);
    Some((stream_id, cursor))
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

    #[test]
    fn acknowledged_frames_have_exact_binary_stream_and_big_endian_cursor() {
        let stream = uuid::Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap();
        let cursor = 0x0102_0304_0506_0708;
        let bytes = acknowledged_frame(stream, cursor, b"opaque native output");
        assert_eq!(bytes[0], ACKNOWLEDGED_CONVERSATION);
        assert_eq!(&bytes[1..17], stream.as_bytes());
        assert_eq!(&bytes[17..25], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(&bytes[25..], b"opaque native output");
        assert_eq!(
            output_acknowledgement(&bytes[1..25]),
            Some((stream, cursor))
        );
        assert!(output_acknowledgement(&bytes[1..24]).is_none());
        assert!(output_acknowledgement(&bytes[1..26]).is_none());
    }
}

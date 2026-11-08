use crate::failure::Result;

pub(super) fn encode(value: u64) -> [u8; 8] {
    value.to_be_bytes()
}

pub(super) fn decode(value: &[u8]) -> Result<u64> {
    let bytes: [u8; 8] = value.try_into().map_err(|_| super::invalid())?;
    let fence = u64::from_be_bytes(bytes);
    if fence == 0 {
        return Err(super::invalid());
    }
    Ok(fence)
}

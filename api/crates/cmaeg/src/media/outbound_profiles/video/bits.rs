//! Bounded H.264 RBSP reading; no entropy/pixel decoding.
pub(super) fn rbsp(nal: &[u8], kind: u8) -> Option<Vec<u8>> {
    let header = *nal.first()?;
    if header & 0x80 != 0 || header & 31 != kind || nal.len() < 2 {
        return None;
    }
    let mut bytes = Vec::with_capacity(nal.len() - 1);
    let mut zeroes = 0;
    let mut index = 1;
    while index < nal.len() {
        let byte = nal[index];
        if zeroes == 2 {
            if byte < 3 {
                return None;
            }
            if byte == 3 {
                if *nal.get(index + 1)? > 3 {
                    return None;
                }
                zeroes = 0;
                index += 1;
                continue;
            }
        }
        zeroes = if byte == 0 { zeroes + 1 } else { 0 };
        bytes.push(byte);
        index += 1;
    }
    Some(bytes)
}

pub(super) struct Bits<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> Bits<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
    pub fn bits(&mut self, count: u32) -> Option<u32> {
        if count > 32 {
            return None;
        }
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1)
                | u32::from((*self.bytes.get(self.position / 8)? >> (7 - self.position % 8)) & 1);
            self.position += 1;
        }
        Some(value)
    }
    pub fn flag(&mut self) -> Option<bool> {
        Some(self.bits(1)? != 0)
    }
    pub fn ue(&mut self, max: u32) -> Option<u32> {
        let mut zeroes = 0;
        while !self.flag()? {
            zeroes += 1;
            if zeroes > 31 {
                return None;
            }
        }
        let value = ((1u64 << zeroes) - 1).checked_add(u64::from(self.bits(zeroes)?))?;
        (value <= u64::from(max)).then_some(value as u32)
    }
    pub fn se(&mut self, min: i32, max: i32) -> Option<i32> {
        let value = self.ue(u32::MAX - 1)?;
        let signed = if value & 1 == 0 {
            -i64::from(value / 2)
        } else {
            i64::from(value.div_ceil(2))
        };
        (i64::from(min)..=i64::from(max))
            .contains(&signed)
            .then_some(signed as i32)
    }
    pub fn trailing(&mut self) -> Option<()> {
        if !self.flag()? {
            return None;
        }
        while !self.position.is_multiple_of(8) {
            if self.flag()? {
                return None;
            }
        }
        (self.position == self.bytes.len() * 8).then_some(())
    }
    pub fn payload_remains(&self) -> bool {
        // A CAVLC slice needs data before its RBSP stop bit. This does not decode it.
        let Some(last) = self.bytes.last().copied().filter(|byte| *byte != 0) else {
            return false;
        };
        self.position < self.bytes.len() * 8 - last.trailing_zeros() as usize - 1
    }
}

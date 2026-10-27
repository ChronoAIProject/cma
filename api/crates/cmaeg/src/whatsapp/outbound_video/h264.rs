//! AVC configuration plus every length-prefixed NAL and complete admitted slice header.
use super::{
    bits::{rbsp, Bits},
    parameters::{self, Pps, Sps},
};

pub(super) struct Avc {
    width: usize,
    sps: Sps,
    pps: Pps,
}
fn parameter<'a>(bytes: &'a [u8], position: &mut usize) -> Option<&'a [u8]> {
    let size = bytes.get(*position..position.checked_add(2)?)?;
    let length = usize::from(u16::from_be_bytes(size.try_into().ok()?));
    *position += 2;
    let end = position.checked_add(length)?;
    let value = bytes.get(*position..end)?;
    *position = end;
    Some(value)
}
impl Avc {
    pub fn parse(bytes: &[u8], width: u32, height: u32) -> Option<Self> {
        if bytes.len() > 4096
            || bytes.first()? != &1
            || bytes.get(4)? & 0xfc != 0xfc
            || bytes.get(5)? != &0xe1
        {
            return None;
        }
        let length_width = usize::from((bytes[4] & 3) + 1);
        if length_width == 3 {
            return None;
        }
        let mut position = 6;
        let sps = parameters::sps(parameter(bytes, &mut position)?, bytes.get(1..4)?)?;
        if sps.width != width || sps.height != height || bytes.get(position)? != &1 {
            return None;
        }
        position += 1;
        let pps = parameters::pps(parameter(bytes, &mut position)?, &sps)?;
        if position != bytes.len() {
            return None;
        }
        Some(Self {
            width: length_width,
            sps,
            pps,
        })
    }
    /// Returns whether the single coded picture is IDR; stss must agree.
    pub fn sample(&self, bytes: &[u8]) -> Option<bool> {
        let mut position = 0usize;
        let mut picture = None;
        let mut count = 0;
        while position < bytes.len() {
            count += 1;
            if count > 256 {
                return None;
            }
            let mut size = 0usize;
            for byte in bytes.get(position..position.checked_add(self.width)?)? {
                size = size.checked_mul(256)?.checked_add(usize::from(*byte))?;
            }
            position += self.width;
            let end = position.checked_add(size)?;
            let nal = bytes.get(position..end)?;
            let header = *nal.first()?;
            let kind = header & 31;
            let data = rbsp(nal, kind)?;
            // A sample is one access unit, one slice; no hidden in-band configuration.
            if picture.is_some() {
                return None;
            }
            match kind {
                1 | 5 => {
                    self.slice(&data, header)?;
                    picture = Some(kind == 5);
                }
                6 if header & 0x60 == 0 => sei(&data)?,
                9 if count == 1 && header & 0x60 == 0 => {
                    let mut bits = Bits::new(&data);
                    if bits.bits(3)? > 1 {
                        return None;
                    }
                    bits.trailing()?;
                }
                _ => return None,
            }
            position = end;
        }
        picture
    }
    fn slice(&self, data: &[u8], header: u8) -> Option<()> {
        let mut bits = Bits::new(data);
        if bits.ue(0)? != 0 {
            return None;
        }
        let kind = bits.ue(9)? % 5;
        if !matches!(kind, 0 | 2) || bits.ue(255)? != self.pps.id {
            return None;
        }
        let frame = bits.bits(self.sps.frame_bits)?;
        let idr = header & 31 == 5;
        if idr {
            if header & 0x60 == 0 || kind != 2 || frame != 0 {
                return None;
            }
            bits.ue(65535)?;
        }
        if let Some(width) = self.sps.poc_bits {
            bits.bits(width)?;
            if self.pps.bottom_poc {
                bits.se(-65535, 65535)?;
            }
        }
        if kind == 0 {
            if bits.flag()? {
                bits.ue(31)?;
            }
            // No reference-list modifications in the initial supported subset.
            if bits.flag()? {
                return None;
            }
        }
        if header & 0x60 != 0 {
            if idr {
                bits.flag()?;
                bits.flag()?;
            } else if bits.flag()? {
                return None;
            } // no adaptive reference marking
        }
        let qp = self.pps.initial_qp + bits.se(-51, 51)?;
        if !(0..=51).contains(&qp) {
            return None;
        }
        if self.pps.deblock && bits.ue(2)? != 1 {
            bits.se(-6, 6)?;
            bits.se(-6, 6)?;
        }
        bits.payload_remains().then_some(())
    }
}
fn sei(data: &[u8]) -> Option<()> {
    let mut position = 0;
    let mut count = 0;
    while data.get(position..) != Some(&[0x80]) {
        count += 1;
        if count > 256 {
            return None;
        }
        for is_size in [false, true] {
            let mut value = 0usize;
            loop {
                let byte = *data.get(position)?;
                position += 1;
                value = value.checked_add(usize::from(byte))?;
                if byte != 255 {
                    break;
                }
            }
            if is_size {
                position = position.checked_add(value)?;
            }
        }
        if position >= data.len() {
            return None;
        }
    }
    Some(())
}

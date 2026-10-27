//! Deliberately narrow progressive Baseline parameter sets (one SPS and PPS).
use super::bits::{rbsp, Bits};

pub(super) struct Sps {
    pub id: u32,
    pub frame_bits: u32,
    pub poc_bits: Option<u32>,
    pub width: u32,
    pub height: u32,
}
pub(super) struct Pps {
    pub id: u32,
    pub bottom_poc: bool,
    pub initial_qp: i32,
    pub deblock: bool,
}
pub(super) fn sps(nal: &[u8], configuration: &[u8]) -> Option<Sps> {
    if nal.first()? & 0x60 == 0 || nal.get(1..4)? != configuration || nal.len() > 1024 {
        return None;
    }
    let data = rbsp(nal, 7)?;
    let mut bits = Bits::new(&data);
    if bits.bits(8)? != 66
        || bits.bits(8)? & 3 != 0
        || !matches!(bits.bits(8)?, 10..=13 | 20..=22 | 30..=32 | 40..=41)
    {
        return None;
    }
    let id = bits.ue(31)?;
    let frame_bits = bits.ue(12)? + 4;
    let poc_bits = match bits.ue(2)? {
        0 => Some(bits.ue(12)? + 4),
        2 => None,
        _ => return None,
    };
    let refs = bits.ue(16)?;
    if bits.flag()? {
        return None;
    } // no frame-number gaps
    let coded_width = (bits.ue(119)? + 1) * 16;
    let coded_height = (bits.ue(67)? + 1) * 16;
    if !bits.flag()? {
        return None;
    } // progressive frames only
    bits.flag()?; // direct_8x8_inference_flag
    let (horizontal, vertical) = if bits.flag()? {
        (
            bits.ue(960)?.checked_add(bits.ue(960)?)?.checked_mul(2)?,
            bits.ue(544)?.checked_add(bits.ue(544)?)?.checked_mul(2)?,
        )
    } else {
        (0, 0)
    };
    let width = coded_width.checked_sub(horizontal)?;
    let height = coded_height.checked_sub(vertical)?;
    if width == 0 || height == 0 || width > super::MAX_WIDTH || height > super::MAX_HEIGHT {
        return None;
    }
    if bits.flag()? {
        vui(&mut bits, refs)?;
    }
    bits.trailing()?;
    Some(Sps {
        id,
        frame_bits,
        poc_bits,
        width,
        height,
    })
}
fn vui(bits: &mut Bits<'_>, refs: u32) -> Option<()> {
    if bits.flag()? {
        match bits.bits(8)? {
            1 => (),
            255 => {
                let horizontal = bits.bits(16)?;
                if horizontal == 0 || horizontal != bits.bits(16)? {
                    return None;
                }
            }
            _ => return None,
        }
    }
    if bits.flag()? {
        bits.flag()?;
    }
    if bits.flag()? {
        if bits.bits(3)? > 5 {
            return None;
        }
        bits.flag()?;
        if bits.flag()? {
            bits.bits(24)?;
        }
    }
    if bits.flag()? {
        bits.ue(5)?;
        bits.ue(5)?;
    }
    if bits.flag()? {
        let tick = bits.bits(32)?;
        let scale = bits.bits(32)?;
        if tick == 0 || scale == 0 || u64::from(scale) > u64::from(tick) * 120 {
            return None;
        }
        bits.flag()?;
    }
    // HRD and picture-structure extensions are outside this profile.
    if bits.flag()? || bits.flag()? || bits.flag()? {
        return None;
    }
    if bits.flag()? {
        bits.flag()?;
        bits.ue(16)?;
        bits.ue(16)?;
        bits.ue(16)?;
        bits.ue(16)?;
        if bits.ue(16)? != 0 || bits.ue(16)? < refs {
            return None;
        }
    }
    Some(())
}
pub(super) fn pps(nal: &[u8], sps: &Sps) -> Option<Pps> {
    if nal.first()? & 0x60 == 0 || nal.len() > 1024 {
        return None;
    }
    let data = rbsp(nal, 8)?;
    let mut bits = Bits::new(&data);
    let id = bits.ue(255)?;
    if bits.ue(31)? != sps.id || bits.flag()? {
        return None;
    } // CAVLC, no CABAC
    let bottom_poc = bits.flag()?;
    if bits.ue(7)? != 0 {
        return None;
    } // no slice groups
    bits.ue(31)?;
    bits.ue(31)?;
    if bits.flag()? || bits.bits(2)? != 0 {
        return None;
    } // no weighted prediction
    let initial_qp = 26 + bits.se(-26, 25)?;
    bits.se(-26, 25)?;
    bits.se(-12, 12)?;
    let deblock = bits.flag()?;
    bits.flag()?;
    if bits.flag()? {
        return None;
    } // no redundant pictures
    bits.trailing()?; // no transform/scaling extensions
    Some(Pps {
        id,
        bottom_poc,
        initial_qp,
        deblock,
    })
}

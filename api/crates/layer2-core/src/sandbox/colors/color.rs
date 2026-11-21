use std::fmt;

use serde::{Deserialize, Serialize};

/// One Sandbox's display colour: an sRGB value spelled as lowercase `#rrggbb`.
///
/// A single spelling keeps stored, compared and published values identical, so
/// two records can never disagree about whether they hold the same colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SandboxColor([u8; 3]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("sandbox colour must be spelled as lowercase #rrggbb")]
pub struct InvalidSandboxColor;

impl SandboxColor {
    pub const fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        Self([red, green, blue])
    }

    pub fn parse(value: &str) -> Result<Self, InvalidSandboxColor> {
        let digits = value.strip_prefix('#').ok_or(InvalidSandboxColor)?;
        if digits.len() != 6
            || !digits
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(InvalidSandboxColor);
        }
        let channel = |at: usize| {
            u8::from_str_radix(&digits[at..at + 2], 16).map_err(|_| InvalidSandboxColor)
        };
        Ok(Self([channel(0)?, channel(2)?, channel(4)?]))
    }

    pub const fn rgb(self) -> [u8; 3] {
        self.0
    }
}

impl fmt::Display for SandboxColor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [red, green, blue] = self.0;
        write!(formatter, "#{red:02x}{green:02x}{blue:02x}")
    }
}

impl TryFrom<String> for SandboxColor {
    type Error = InvalidSandboxColor;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<SandboxColor> for String {
    fn from(value: SandboxColor) -> Self {
        value.to_string()
    }
}

/// A colour in OKLab, where Euclidean distance tracks how different two
/// colours look; sRGB distance overstates differences between greens and
/// understates them between blues.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Oklab {
    pub(super) lightness: f64,
    a: f64,
    b: f64,
}

impl Oklab {
    pub(super) fn chroma(self) -> f64 {
        self.a.hypot(self.b)
    }

    pub(super) fn distance(self, other: Self) -> f64 {
        ((self.lightness - other.lightness).powi(2)
            + (self.a - other.a).powi(2)
            + (self.b - other.b).powi(2))
        .sqrt()
    }
}

impl From<SandboxColor> for Oklab {
    // Björn Ottosson's published sRGB -> OKLab matrices.
    fn from(color: SandboxColor) -> Self {
        let [red, green, blue] = color.0.map(linear);
        let l = (0.412_221_470_8 * red + 0.536_332_536_3 * green + 0.051_445_992_9 * blue).cbrt();
        let m = (0.211_903_498_2 * red + 0.680_699_545_1 * green + 0.107_396_956_6 * blue).cbrt();
        let s = (0.088_302_461_9 * red + 0.281_718_837_6 * green + 0.629_978_700_5 * blue).cbrt();
        Self {
            lightness: 0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
            a: 1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
            b: 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
        }
    }
}

fn linear(channel: u8) -> f64 {
    let value = f64::from(channel) / 255.0;
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

use super::color::{Oklab, SandboxColor};

/// The two status colours a character switches to while it waits for approval
/// or has failed. A Sandbox colour near either would read as that status.
const RESERVED: [SandboxColor; 2] = [
    SandboxColor::from_rgb(0xff, 0xc4, 0x5a),
    SandboxColor::from_rgb(0xbc, 0x3b, 0x26),
];
const RESERVED_MARGIN: f64 = 0.15;
/// Lighter bodies wash out on the light theme and darker ones sink into the
/// dark theme; below this chroma a body reads as grey rather than a colour.
const LIGHTNESS: std::ops::RangeInclusive<f64> = 0.62..=0.86;
const MIN_CHROMA: f64 = 0.10;
/// More candidates spread colours further apart; 64 keeps 15–18 Sandboxes at
/// least ΔEok 0.10 apart for a few hundred random bytes.
const CANDIDATES: usize = 64;
/// About a third of random sRGB values are admissible, so a healthy source
/// needs roughly 200 draws; the bound only stops a broken source from spinning.
const MAX_DRAWS: usize = CANDIDATES * 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PaletteError {
    #[error("the random source failed")]
    RandomUnavailable,
    #[error("the random source produced no admissible colour")]
    Exhausted,
}

/// Where new colours come from, injected so tests can replay a choice.
pub trait ColorRandom: Send + Sync {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), PaletteError>;
}

/// The operating system's secure generator; colours need no secrecy, but this
/// is the generator the crate already trusts and it cannot be seeded by input.
pub struct SystemColorRandom(ring::rand::SystemRandom);

impl SystemColorRandom {
    pub fn new() -> Self {
        Self(ring::rand::SystemRandom::new())
    }
}

impl Default for SystemColorRandom {
    fn default() -> Self {
        Self::new()
    }
}

impl ColorRandom for SystemColorRandom {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), PaletteError> {
        ring::rand::SecureRandom::fill(&self.0, bytes).map_err(|_| PaletteError::RandomUnavailable)
    }
}

/// Whether `color` belongs to the pool Sandbox colours are drawn from.
pub fn admissible(color: SandboxColor) -> bool {
    let lab = Oklab::from(color);
    LIGHTNESS.contains(&lab.lightness)
        && lab.chroma() >= MIN_CHROMA
        && RESERVED
            .iter()
            .all(|reserved| lab.distance(Oklab::from(*reserved)) >= RESERVED_MARGIN)
}

/// A random admissible colour that looks as different as possible from every
/// `occupied` colour. When the pool is crowded the result may sit close to a
/// neighbour, but it is always the most distinct of the candidates drawn.
pub fn choose(
    occupied: &[SandboxColor],
    random: &dyn ColorRandom,
) -> Result<SandboxColor, PaletteError> {
    let occupied: Vec<Oklab> = occupied.iter().copied().map(Oklab::from).collect();
    let mut best: Option<(f64, SandboxColor)> = None;
    let mut found = 0;
    let mut bytes = [0_u8; CANDIDATES * 3];
    let mut draws = 0;
    while found < CANDIDATES {
        if draws >= MAX_DRAWS {
            return best.map(|(_, color)| color).ok_or(PaletteError::Exhausted);
        }
        random.fill(&mut bytes)?;
        for rgb in bytes.chunks_exact(3) {
            draws += 1;
            let candidate = SandboxColor::from_rgb(rgb[0], rgb[1], rgb[2]);
            if !admissible(candidate) {
                continue;
            }
            found += 1;
            let spacing = nearest(Oklab::from(candidate), &occupied);
            if best.is_none_or(|(current, _)| spacing > current) {
                best = Some((spacing, candidate));
            }
            if found == CANDIDATES {
                break;
            }
        }
    }
    best.map(|(_, color)| color).ok_or(PaletteError::Exhausted)
}

/// The smallest perceptual distance between `color` and any of `others`;
/// infinite when there are none.
pub fn spacing(color: SandboxColor, others: &[SandboxColor]) -> f64 {
    let others: Vec<Oklab> = others.iter().copied().map(Oklab::from).collect();
    nearest(Oklab::from(color), &others)
}

fn nearest(lab: Oklab, others: &[Oklab]) -> f64 {
    others
        .iter()
        .map(|other| lab.distance(*other))
        .fold(f64::INFINITY, f64::min)
}

//! The seeded generator behind the Lark extractors' invariant tests.

/// Seeded xorshift64, so a failing case is reproducible from its number.
pub(super) struct Rng(pub(super) u64);

impl Rng {
    pub(super) fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub(super) fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    pub(super) fn pick<T: Clone>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())].clone()
    }
}

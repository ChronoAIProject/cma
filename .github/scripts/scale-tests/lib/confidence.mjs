import { check, hash } from './checks.mjs';

const percentile = (values, p) => [...values].sort((a, b) => a - b)[Math.ceil(p * values.length) - 1];

// Circular block bootstrap preserves short runs of correlated observations.
// Seed, block size and draw count belong to the preregistered measurement policy.
export function deltaInterval(baseline, candidate, { seed = 'cma-scale-v1', blockSize = 10, draws = 1000 } = {}) {
  check(baseline.length >= 100 && candidate.length >= 100 && blockSize >= 1
    && Number.isSafeInteger(blockSize) && blockSize <= Math.min(baseline.length, candidate.length)
    && Number.isSafeInteger(draws) && draws >= 1000, 'confidence_sample_budget_invalid');
  check([...baseline, ...candidate].every((n) => Number.isFinite(n) && n >= 0), 'confidence_sample_invalid');
  let state = Number.parseInt(hash(seed).slice(0, 8), 16);
  const random = () => { state = (Math.imul(1664525, state) + 1013904223) >>> 0; return state / 4294967296; };
  const resample = (values) => {
    const result = [];
    while (result.length < values.length) {
      const start = Math.floor(random() * values.length);
      for (let i = 0; i < blockSize && result.length < values.length; i++) result.push(values[(start + i) % values.length]);
    }
    return percentile(result, 0.95);
  };
  const differences = Array.from({ length: draws }, () => resample(candidate) - resample(baseline));
  return { lower_ms: percentile(differences, 0.025), upper_ms: percentile(differences, 0.975),
    confidence: 0.95, draws, block_size: blockSize, seed_hash: hash(seed) };
}

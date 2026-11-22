/*!
 * Copyright (c) 2026 ReactDrops. ReactDrops Component License 1.0.
 * Source: crisjavr/reactdrops @ aed0014649a2f0964041ac33c5f02dc3a40c5e13.
 * See LICENSE, NOTICE and source-map.json in this directory.
 */
const fade = (t: number) => t * t * t * (t * (t * 6 - 15) + 10);
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;

export function createElevationField(seed: number) {
  const permutation = new Uint8Array(512);
  const values = Array.from({ length: 256 }, (_, i) => i);
  let state = (Math.round(seed) ^ 0x9e3779b9) >>> 0;
  const random = () => {
    state += 0x6d2b79f5;
    let n = Math.imul(state ^ (state >>> 15), state | 1);
    n ^= n + Math.imul(n ^ (n >>> 7), n | 61);
    return ((n ^ (n >>> 14)) >>> 0) / 4294967296;
  };
  for (let i = 255; i > 0; i--) {
    const j = Math.floor(random() * (i + 1));
    [values[i], values[j]] = [values[j]!, values[i]!];
  }
  for (let i = 0; i < 512; i++) permutation[i] = values[i & 255]!;
  const gradientX = new Float64Array(256),
    gradientY = new Float64Array(256);
  for (let i = 0; i < 256; i++) {
    const angle = random() * Math.PI * 2;
    gradientX[i] = Math.cos(angle);
    gradientY[i] = Math.sin(angle);
  }
  const gradient = (hash: number, x: number, y: number, cosine: number, sine: number) => {
    const gx = gradientX[hash]!,
      gy = gradientY[hash]!;
    return (gx * cosine - gy * sine) * x + (gx * sine + gy * cosine) * y;
  };
  const noise = (x: number, y: number, cosine: number, sine: number) => {
    const ix = Math.floor(x),
      iy = Math.floor(y);
    const fx = x - ix,
      fy = y - iy;
    const a = permutation[ix & 255]! + (iy & 255);
    const b = permutation[(ix + 1) & 255]! + (iy & 255);
    return lerp(
      lerp(
        gradient(permutation[a]!, fx, fy, cosine, sine),
        gradient(permutation[b]!, fx - 1, fy, cosine, sine),
        fade(fx),
      ),
      lerp(
        gradient(permutation[a + 1]!, fx, fy - 1, cosine, sine),
        gradient(permutation[b + 1]!, fx - 1, fy - 1, cosine, sine),
        fade(fx),
      ),
      fade(fy),
    );
  };
  type Rotation = [number, number, number, number];
  const rotation = (phase: number): Rotation => {
    const a = 0.3 * Math.sin(phase),
      b = 0.3 * Math.cos(phase);
    return [Math.cos(a), Math.sin(a), Math.cos(b), Math.sin(b)];
  };
  let previousTime = NaN,
    phase = 0;
  let mainRotation = rotation(0),
    layerRotation = rotation(0.7),
    crossRotation = rotation(-1.3);
  let orbitX = 0,
    orbitY = 0,
    secondX = 0,
    secondY = 0,
    pigmentX = 0,
    pigmentY = 0;
  const morph = (x: number, y: number, r: Rotation) =>
    0.96 * noise(x, y, r[0], r[1]) +
    0.12 * noise(0.8 * x - 0.6 * y + 29, 0.6 * x + 0.8 * y - 17, r[2], r[3]);
  return (x: number, y: number, time: number, distortion: number) => {
    if (time !== previousTime) {
      previousTime = time;
      // Grain Gradient's Drift cadence: a slow closed cycle with continuous velocity.
      phase = ((time + 18) * 0.02) % (Math.PI * 2);
      mainRotation = rotation(phase);
      layerRotation = rotation(phase + 0.7);
      crossRotation = rotation(phase - 1.3);
      orbitX = Math.cos(phase);
      orbitY = Math.sin(phase);
      secondX = Math.cos(2 * phase);
      secondY = Math.sin(2 * phase);
      pigmentX = 0.32 * (secondX - 1);
      pigmentY = 0.32 * secondY;
    }
    // Two broad, evolving domains carry the hills together. Small detail shares
    // the same phase, rather than animating each contour independently.
    const frequency = 1.25;
    const reach = 0.23 * distortion;
    let bendX = 0,
      bendY = 0;
    for (let layer = 0; layer < 2; layer++) {
      const px = x * frequency + bendX + 17 * layer;
      const py = y * frequency + bendY - 9 * layer;
      const ox = layer ? secondX : orbitX,
        oy = layer ? secondY : orbitY;
      bendX = reach * morph(px + 0.55 * ox, py + 0.55 * oy, layer ? layerRotation : mainRotation);
      bendY =
        reach *
        morph(
          0.6 * px - 0.8 * py - 13 - 0.42 * ox,
          0.8 * px + 0.6 * py + 8 - 0.42 * oy,
          crossRotation,
        );
    }
    return (
      1.6 * morph(x * frequency + bendX + pigmentX, y * frequency + bendY + pigmentY, mainRotation)
    );
  };
}

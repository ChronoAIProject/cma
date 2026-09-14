import * as THREE from 'three';

const RADIUS = 0.355;
const HEM_LOW = -0.262;
const HEM_HIGH = -0.052;
const SCALLOPS = 6;
const RADIAL_SEGMENTS = 128;
const BODY_ROWS = 36;
const LIP_ROWS = 4;
const ROWS_PER_RING = LIP_ROWS + BODY_ROWS;

export const faceRadius = (y: number) => Math.sqrt(Math.max(RADIUS * RADIUS - y * y, 1e-6));
export const clamp = (value: number, lower: number, upper: number) =>
  Math.min(upper, Math.max(lower, value));

export function roundedRect(width: number, height: number, radius: number) {
  const shape = new THREE.Shape();
  shape.moveTo(-width / 2 + radius, -height / 2);
  shape.lineTo(width / 2 - radius, -height / 2);
  shape.quadraticCurveTo(width / 2, -height / 2, width / 2, -height / 2 + radius);
  shape.lineTo(width / 2, height / 2 - radius);
  shape.quadraticCurveTo(width / 2, height / 2, width / 2 - radius, height / 2);
  shape.lineTo(-width / 2 + radius, height / 2);
  shape.quadraticCurveTo(-width / 2, height / 2, -width / 2, height / 2 - radius);
  shape.lineTo(-width / 2, -height / 2 + radius);
  shape.quadraticCurveTo(-width / 2, -height / 2, -width / 2 + radius, -height / 2);
  return shape;
}

export function wrap(
  geometry: THREE.BufferGeometry,
  azimuthCentre: number,
  yCentre: number,
  lift: number,
) {
  const position = geometry.getAttribute('position');
  const vertex = new THREE.Vector3();
  for (let index = 0; index < position.count; index += 1) {
    vertex.fromBufferAttribute(position, index);
    const y = vertex.y + yCentre;
    const azimuth = azimuthCentre + vertex.x / faceRadius(yCentre);
    const radius = faceRadius(y) + vertex.z + lift;
    position.setXYZ(index, radius * Math.sin(azimuth), y, radius * Math.cos(azimuth));
  }
  position.needsUpdate = true;
  geometry.computeVertexNormals();
  return geometry;
}

export function makeBody(material: THREE.Material) {
  const positions = new Float32Array((RADIAL_SEGMENTS + 1) * ROWS_PER_RING * 3 + 3);
  const indices: number[] = [];
  for (let radial = 0; radial < RADIAL_SEGMENTS; radial += 1) {
    for (let row = 0; row < ROWS_PER_RING - 1; row += 1) {
      const current = radial * ROWS_PER_RING + row;
      const next = (radial + 1) * ROWS_PER_RING + row;
      indices.push(current, next, current + 1, next, next + 1, current + 1);
    }
  }

  const apex = (RADIAL_SEGMENTS + 1) * ROWS_PER_RING;
  positions[apex * 3 + 1] = -0.14;
  for (let radial = 0; radial < RADIAL_SEGMENTS; radial += 1) {
    indices.push(apex, (radial + 1) * ROWS_PER_RING, radial * ROWS_PER_RING);
  }

  const geometry = new THREE.BufferGeometry();
  geometry.setAttribute('position', new THREE.BufferAttribute(positions, 3));
  geometry.setIndex(indices);

  const trig: [number, number, number][] = [];
  for (let radial = 0; radial <= RADIAL_SEGMENTS; radial += 1) {
    const theta = (radial / RADIAL_SEGMENTS) * Math.PI * 2;
    trig.push([theta, Math.sin(theta), Math.cos(theta)]);
  }
  const easing: number[] = [];
  for (let row = 0; row < BODY_ROWS; row += 1) {
    const progress = row / (BODY_ROWS - 1);
    easing.push(progress * progress * (3 - 2 * progress));
  }

  function update(time: number) {
    const breathe = 1 + 0.055 * Math.sin(time * 0.9);
    for (let radial = 0; radial <= RADIAL_SEGMENTS; radial += 1) {
      const point = trig[radial];
      if (point === undefined) {
        continue;
      }
      const [theta, sine, cosine] = point;
      const scallop = 0.5 - 0.5 * Math.cos(SCALLOPS * theta);
      const rimY =
        HEM_LOW +
        (HEM_HIGH - HEM_LOW) * scallop * breathe +
        0.014 * Math.sin(4 * theta - time * 1.9);
      const rimRadius = faceRadius(rimY);
      let offset = radial * ROWS_PER_RING * 3;
      const put = (radius: number, y: number) => {
        positions[offset] = radius * sine;
        positions[offset + 1] = y;
        positions[offset + 2] = radius * cosine;
        offset += 3;
      };

      put(rimRadius * 0.58, rimY + 0.052);
      put(rimRadius * 0.85, rimY + 0.024);
      put(rimRadius - 0.02, rimY + 0.003);
      put(rimRadius - 0.005, rimY - 0.005);
      const firstPhi = Math.asin(clamp(rimY / RADIUS, -1, 1));
      const span = Math.PI / 2 - firstPhi;
      for (let row = 0; row < BODY_ROWS; row += 1) {
        const eased = easing[row];
        if (eased === undefined) {
          continue;
        }
        const phi = firstPhi + span * eased;
        put(RADIUS * Math.cos(phi), RADIUS * Math.sin(phi));
      }
    }
    geometry.getAttribute('position').needsUpdate = true;
  }

  update(0);
  geometry.computeVertexNormals();
  const mesh = new THREE.Mesh(geometry, material);
  mesh.name = 'body';
  return { mesh, update };
}

export function shadowTexture() {
  const canvas = document.createElement('canvas');
  canvas.width = 128;
  canvas.height = 128;
  const context = canvas.getContext('2d');
  if (context === null) {
    throw new Error('2D canvas is unavailable');
  }
  const gradient = context.createRadialGradient(64, 64, 0, 64, 64, 64);
  gradient.addColorStop(0, 'rgba(255,255,255,0.82)');
  gradient.addColorStop(0.45, 'rgba(255,255,255,0.28)');
  gradient.addColorStop(1, 'rgba(255,255,255,0)');
  context.fillStyle = gradient;
  context.fillRect(0, 0, 128, 128);
  return new THREE.CanvasTexture(canvas);
}

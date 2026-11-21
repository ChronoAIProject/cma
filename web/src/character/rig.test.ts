import * as THREE from 'three';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { CharacterPalette } from '../landing/characterPalette';
import { createCharacterRig, REST_FRAME } from './rig';

beforeEach(() => {
  // The contact shadow paints a radial gradient on a 2D canvas, which jsdom does not draw.
  vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue({
    createRadialGradient: () => ({ addColorStop: vi.fn() }),
    fillRect: vi.fn(),
    fillStyle: '',
  } as unknown as CanvasRenderingContext2D);
});

afterEach(() => {
  vi.restoreAllMocks();
});

function parts(rig: ReturnType<typeof createCharacterRig>) {
  const body = rig.scene.getObjectByName('body') as THREE.Mesh;
  const head = body.parent as THREE.Group;
  const group = head.parent as THREE.Group;
  const sprite = rig.scene.children.find((child) => child instanceof THREE.Sprite) as THREE.Sprite;
  const glints: THREE.Mesh[] = [];
  head.traverse((child) => {
    if (
      child instanceof THREE.Mesh &&
      child.material instanceof THREE.MeshBasicMaterial &&
      child.renderOrder === 2
    )
      glints.push(child);
  });
  return { body, head, group, sprite, glints };
}

describe('createCharacterRig', () => {
  it('builds the shipped body, two lenses, a bridge, two glints and a contact shadow', () => {
    const rig = createCharacterRig();
    expect(rig.scene.getObjectByName('body')).toBeInstanceOf(THREE.Mesh);
    expect(rig.scene.getObjectByName('lens-left')).toBeInstanceOf(THREE.Mesh);
    expect(rig.scene.getObjectByName('lens-right')).toBeInstanceOf(THREE.Mesh);
    expect(rig.scene.getObjectByName('bridge')).toBeInstanceOf(THREE.Mesh);
    const { glints, sprite } = parts(rig);
    expect(glints).toHaveLength(2);
    expect(sprite.visible).toBe(true);
  });

  it('applies every rig channel of a frame', () => {
    const rig = createCharacterRig();
    const { head, group, body, sprite } = parts(rig);
    rig.applyFrame({
      ...REST_FRAME,
      headYaw: 0.3,
      headPitch: -0.2,
      swing: 0.1,
      y: 0.05,
      rotX: 0.1,
      rotY: 1,
      rotZ: -0.05,
      scale: [1.1, 0.9, 1.1],
    });
    expect(head.rotation.y).toBeCloseTo(0.3);
    expect(head.rotation.x).toBeCloseTo(-0.2);
    expect(body.rotation.y).toBeCloseTo(-0.05);
    expect(body.rotation.z).toBeCloseTo(0.022);
    expect(group.position.y).toBeCloseTo(0.05);
    expect(group.rotation.y).toBeCloseTo(1);
    expect(group.scale.y).toBeCloseTo(0.9);
    expect((sprite.material as THREE.SpriteMaterial).opacity).toBeCloseTo(0.46 - 0.07);
    expect(sprite.scale.x).toBeCloseTo(0.76 * (1 - 0.025));
  });

  it('shows the lens glint only while one is in progress', () => {
    const rig = createCharacterRig();
    const { glints } = parts(rig);
    const opacity = () => (glints[0]?.material as THREE.MeshBasicMaterial).opacity;
    rig.applyFrame({ ...REST_FRAME, glint: 0.5 });
    expect(opacity()).toBeCloseTo(0.75);
    rig.applyFrame({ ...REST_FRAME, glint: 1.4 });
    expect(opacity()).toBeCloseTo(0);
    rig.applyFrame(REST_FRAME);
    expect(opacity()).toBe(0);
  });

  it('wears the palette on body, lenses, lights and shadow', () => {
    const rig = createCharacterRig();
    const color = (hex: string) => new THREE.Color(hex);
    const palette: CharacterPalette = {
      body: color('#c3fd43'),
      lens: color('#09090b'),
      sky: color('#fafafa'),
      ground: color('#27272a'),
      key: color('#fafafa'),
      fill: color('#c3fd43'),
      rim: color('#a1a1aa'),
      glint: color('#fafafa'),
      shadow: color('#ff0000'),
    };
    rig.applyPalette(palette);
    const { body, sprite, glints } = parts(rig);
    expect((body.material as THREE.MeshPhysicalMaterial).color.getHexString()).toBe('c3fd43');
    expect((sprite.material as THREE.SpriteMaterial).color.getHexString()).toBe('ff0000');
    expect((glints[0]?.material as THREE.MeshBasicMaterial).color.getHexString()).toBe('fafafa');
  });

  it('hides the shadow and follows the canvas aspect on request', () => {
    const rig = createCharacterRig();
    rig.setShadow(false);
    expect(parts(rig).sprite.visible).toBe(false);
    rig.setAspect(2);
    expect(rig.camera.aspect).toBe(2);
  });

  it('releases its geometry, materials and texture', () => {
    const rig = createCharacterRig();
    const geometry = vi.spyOn(THREE.BufferGeometry.prototype, 'dispose');
    const texture = vi.spyOn(THREE.Texture.prototype, 'dispose');
    rig.dispose();
    expect(geometry).toHaveBeenCalled();
    expect(texture).toHaveBeenCalled();
    expect(rig.scene.children).toHaveLength(0);
  });
});

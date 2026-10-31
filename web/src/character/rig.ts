import * as THREE from 'three';
import {
  faceRadius,
  makeBody,
  roundedRect,
  shadowTexture,
  wrap,
} from '../landing/characterGeometry';
import type { CharacterPalette } from '../landing/characterPalette';

const EYE_Y = 0.06;
const LENS_HEIGHT = 0.12;
const LENS_DEPTH = 0.032;
const LENS_RADIUS = 0.04;
const LENS_GAP = 0.03;
const LENS_SPAN = 0.2;
const SHADOW_OPACITY = 0.46;

/**
 * One rendered moment of the character, in the rig's own channels. Every surface that
 * draws the character — the landing stage and the shared avatar stage — reduces its
 * motion to these values, so a pose looks identical wherever it is shown.
 */
export interface RigFrame {
  /** Head turn and nod, in radians (the landing pointer look drives the same pair). */
  readonly headYaw: number;
  readonly headPitch: number;
  /** Trailing hem swing that follows a change in head yaw. */
  readonly swing: number;
  /** The body's breathing clock in seconds; 0 keeps the hem at rest. */
  readonly hem: number;
  readonly y: number;
  readonly rotX: number;
  readonly rotY: number;
  readonly rotZ: number;
  readonly scale: readonly [number, number, number];
  /** Lens glint progress in [0, 1), or a negative value when no glint is showing. */
  readonly glint: number;
}

export const REST_FRAME: RigFrame = {
  headYaw: 0,
  headPitch: 0,
  swing: 0,
  hem: 0,
  y: 0,
  rotX: 0,
  rotY: 0,
  rotZ: 0,
  scale: [1, 1, 1],
  glint: -1,
};

export interface CharacterRig {
  readonly scene: THREE.Scene;
  readonly camera: THREE.PerspectiveCamera;
  /** Show or hide the contact shadow; compact characters omit it. */
  setShadow(visible: boolean): void;
  applyPalette(palette: CharacterPalette): void;
  applyFrame(frame: RigFrame): void;
  /** Point the camera at a canvas of this aspect ratio. */
  setAspect(aspect: number): void;
  dispose(): void;
}

/**
 * Build the shipped character: geometry from `characterGeometry`, the MeshPhysical body and
 * lenses, studio lights, lens glints and the contact shadow. Nothing here schedules frames
 * or owns a renderer.
 */
export function createCharacterRig(): CharacterRig {
  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(32, 1, 0.1, 20);
  camera.position.set(0, 0.02, 1.75);
  camera.lookAt(0, -0.03, 0);

  const bodyMaterial = new THREE.MeshPhysicalMaterial({
    name: 'cma-body',
    roughness: 0.34,
    metalness: 0.04,
    clearcoat: 0.35,
    clearcoatRoughness: 0.28,
    sheen: 0.7,
    sheenRoughness: 0.45,
  });
  const lensMaterial = new THREE.MeshPhysicalMaterial({
    name: 'cma-lenses',
    roughness: 0.06,
    metalness: 0.05,
    clearcoat: 1,
    clearcoatRoughness: 0.03,
  });

  const hemisphere = new THREE.HemisphereLight();
  hemisphere.intensity = 1.25;
  scene.add(hemisphere);
  const key = new THREE.DirectionalLight();
  key.intensity = 2.65;
  key.position.set(1.4, 2, 2.2);
  scene.add(key);
  const fill = new THREE.DirectionalLight();
  fill.intensity = 0.72;
  fill.position.set(-2, -0.4, 1.1);
  scene.add(fill);
  const rim = new THREE.DirectionalLight();
  rim.intensity = 1.8;
  rim.position.set(-1.2, 1.1, -2);
  scene.add(rim);

  const rig = new THREE.Group();
  const head = new THREE.Group();
  rig.add(head);
  scene.add(rig);

  const body = makeBody(bodyMaterial);
  body.mesh.scale.set(1.035, 0.965, 1.035);
  head.add(body.mesh);

  const face = faceRadius(EYE_Y);
  const centreX = (LENS_GAP + LENS_SPAN) / 2;
  const glints: {
    readonly pivot: THREE.Object3D;
    readonly material: THREE.MeshBasicMaterial;
    readonly base: number;
  }[] = [];

  for (const side of [-1, 1]) {
    const lensGeometry = new THREE.ExtrudeGeometry(
      roundedRect(LENS_SPAN - LENS_GAP, LENS_HEIGHT, LENS_RADIUS),
      {
        depth: LENS_DEPTH,
        bevelEnabled: true,
        bevelSize: 0.008,
        bevelThickness: 0.008,
        bevelSegments: 5,
        curveSegments: 20,
      },
    );
    lensGeometry.center();
    wrap(lensGeometry, (side * centreX) / face, EYE_Y, LENS_DEPTH / 2);
    const lens = new THREE.Mesh(lensGeometry, lensMaterial);
    lens.name = side < 0 ? 'lens-left' : 'lens-right';
    head.add(lens);

    const glintGeometry = new THREE.ExtrudeGeometry(roundedRect(0.026, LENS_HEIGHT * 0.78, 0.012), {
      depth: 0.002,
      bevelEnabled: false,
      curveSegments: 8,
    });
    glintGeometry.center();
    wrap(glintGeometry, 0, EYE_Y, LENS_DEPTH + 0.006);
    const glintMaterial = new THREE.MeshBasicMaterial({
      transparent: true,
      opacity: 0,
      depthWrite: false,
    });
    const glint = new THREE.Mesh(glintGeometry, glintMaterial);
    glint.renderOrder = 2;
    const pivot = new THREE.Object3D();
    pivot.add(glint);
    head.add(pivot);
    glints.push({ pivot, material: glintMaterial, base: (side * centreX) / face });
  }

  const bridgeArc = (LENS_GAP * 2.2) / face;
  const bridgeGeometry = new THREE.TorusGeometry(
    face + LENS_DEPTH * 0.62,
    0.019,
    16,
    48,
    bridgeArc,
  );
  bridgeGeometry.rotateX(Math.PI / 2);
  bridgeGeometry.rotateY(bridgeArc / 2 - Math.PI / 2);
  bridgeGeometry.translate(0, EYE_Y, 0);
  const bridge = new THREE.Mesh(bridgeGeometry, lensMaterial);
  bridge.name = 'bridge';
  head.add(bridge);

  const shadowMap = shadowTexture();
  const shadowMaterial = new THREE.SpriteMaterial({
    map: shadowMap,
    transparent: true,
    opacity: SHADOW_OPACITY,
    depthWrite: false,
  });
  const shadow = new THREE.Sprite(shadowMaterial);
  shadow.scale.set(0.76, 0.11, 1);
  shadow.position.set(0, -0.43, -0.05);
  scene.add(shadow);

  return {
    scene,
    camera,
    setShadow(visible) {
      shadow.visible = visible;
    },
    applyPalette(palette) {
      bodyMaterial.color.copy(palette.body);
      bodyMaterial.sheenColor.copy(palette.key);
      lensMaterial.color.copy(palette.lens);
      hemisphere.color.copy(palette.sky);
      hemisphere.groundColor.copy(palette.ground);
      key.color.copy(palette.key);
      fill.color.copy(palette.fill);
      rim.color.copy(palette.rim);
      shadowMaterial.color.copy(palette.shadow);
      for (const glint of glints) glint.material.color.copy(palette.glint);
    },
    applyFrame(frame) {
      body.update(frame.hem);
      head.rotation.y = frame.headYaw;
      head.rotation.x = frame.headPitch;
      body.mesh.rotation.y = -frame.swing * 0.5;
      body.mesh.rotation.z = frame.swing * 0.22;
      rig.position.y = frame.y;
      rig.rotation.set(frame.rotX, frame.rotY, frame.rotZ);
      rig.scale.set(frame.scale[0], frame.scale[1], frame.scale[2]);

      shadowMaterial.opacity = SHADOW_OPACITY - frame.y * 1.4;
      const shadowScale = 1 - frame.y * 0.5;
      shadow.scale.set(0.76 * shadowScale, 0.11 * shadowScale, 1);

      for (const glint of glints) {
        if (frame.glint >= 0) {
          glint.pivot.rotation.y = glint.base + (frame.glint - 0.5) * 0.34;
          glint.material.opacity = Math.sin(Math.min(frame.glint, 1) * Math.PI) * 0.75;
        } else {
          glint.material.opacity = 0;
        }
      }
    },
    setAspect(aspect) {
      camera.aspect = aspect;
      camera.updateProjectionMatrix();
    },
    dispose() {
      const geometries = new Set<THREE.BufferGeometry>();
      const materials = new Set<THREE.Material>();
      scene.traverse((object) => {
        if (object instanceof THREE.Mesh) {
          geometries.add(object.geometry);
          const owned = Array.isArray(object.material) ? object.material : [object.material];
          for (const material of owned) materials.add(material);
        }
        if (object instanceof THREE.Sprite) materials.add(object.material);
      });
      for (const geometry of geometries) geometry.dispose();
      for (const material of materials) material.dispose();
      shadowMap.dispose();
      scene.clear();
    },
  };
}

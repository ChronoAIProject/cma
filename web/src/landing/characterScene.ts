import * as THREE from 'three';
import { stepMotion } from './characterMotion';

const RADIUS = 0.355;
const HEM_LOW = -0.262;
const HEM_HIGH = -0.052;
const SCALLOPS = 6;
const EYE_Y = 0.06;
const LENS_HEIGHT = 0.12;
const LENS_DEPTH = 0.032;
const LENS_RADIUS = 0.04;
const LENS_GAP = 0.03;
const LENS_SPAN = 0.2;
const RADIAL_SEGMENTS = 128;
const BODY_ROWS = 36;
const LIP_ROWS = 4;
const ROWS_PER_RING = LIP_ROWS + BODY_ROWS;

interface CharacterOptions {
  readonly onReady?: () => void;
  readonly onUnavailable?: () => void;
  readonly state?: 'brand' | 'running' | 'completed' | 'pending' | 'failed';
  readonly autoRotate?: boolean;
  readonly onActivate?: () => void;
}

export interface CharacterHandle {
  readonly dispose: () => void;
}

interface CharacterPalette {
  readonly body: THREE.Color;
  readonly lens: THREE.Color;
  readonly sky: THREE.Color;
  readonly ground: THREE.Color;
  readonly key: THREE.Color;
  readonly fill: THREE.Color;
  readonly rim: THREE.Color;
  readonly glint: THREE.Color;
  readonly shadow: THREE.Color;
}

const faceRadius = (y: number) => Math.sqrt(Math.max(RADIUS * RADIUS - y * y, 1e-6));
const clamp = (value: number, lower: number, upper: number) =>
  Math.min(upper, Math.max(lower, value));

function roundedRect(width: number, height: number, radius: number) {
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

function wrap(
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

function makeBody(material: THREE.Material) {
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

function shadowTexture() {
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

function readPalette(host: HTMLElement, state: CharacterOptions['state']): CharacterPalette {
  const styles = getComputedStyle(host);
  const read = (property: string) => {
    const value = styles.getPropertyValue(property).trim();
    if (value.length === 0) {
      throw new Error(`character colour ${property} did not resolve`);
    }
    return new THREE.Color(value);
  };
  const dark = document.body.hasAttribute('data-ds-dark-theme');

  return {
    body: read(
      state === undefined || state === 'brand'
        ? '--dsw-alias-brand-primary'
        : `--dsw-alias-character-${state}`,
    ),
    lens: read('--dsw-alias-brand-primary-invert'),
    sky: read(dark ? '--dsw-alias-label-primary' : '--dsw-alias-label-primary-foreground'),
    ground: read('--dsw-alias-bg-layer-3'),
    key: read(dark ? '--dsw-alias-label-primary' : '--dsw-alias-label-primary-foreground'),
    fill: read('--dsw-alias-brand-primary'),
    rim: read(dark ? '--dsw-alias-label-secondary' : '--dsw-alias-bg-layer-1'),
    glint: read(dark ? '--dsw-alias-label-primary' : '--dsw-alias-label-primary-foreground'),
    shadow: read(dark ? '--dsw-alias-brand-primary' : '--dsw-alias-label-primary'),
  };
}

/** Mount the supplied procedural CMA character into a transparent, responsive host. */
export function mountCharacter(host: HTMLElement, options: CharacterOptions = {}): CharacterHandle {
  const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
  const decorative = options.state === undefined || options.state === 'brand';
  const moving = decorative || options.state !== 'failed';
  let reduced = motion.matches;
  let disposed = false;
  let intersecting = true;
  let contextAvailable = true;
  let animationFrame: number | null = null;
  let lastFrame: number | null = null;
  let animationTime = 0;
  let ready = false;

  const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
  renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.toneMapping = THREE.ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.12;
  renderer.setClearColor(0x000000, 0);
  renderer.domElement.className = 'landing__character-canvas';
  renderer.domElement.setAttribute('aria-hidden', 'true');
  host.appendChild(renderer.domElement);

  function syncInteraction() {
    const enabled =
      (!reduced || options.onActivate !== undefined) && contextAvailable && decorative;
    host.toggleAttribute('data-motion', enabled);
    if (host instanceof HTMLButtonElement) {
      host.disabled = !enabled;
    }
  }

  syncInteraction();

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
    opacity: 0.46,
    depthWrite: false,
  });
  const shadow = new THREE.Sprite(shadowMaterial);
  shadow.visible = decorative;
  shadow.scale.set(0.76, 0.11, 1);
  shadow.position.set(0, -0.43, -0.05);
  scene.add(shadow);

  const pointer = { x: 0, y: 0 };
  const target = { x: 0, y: 0 };
  let spin = 0;
  let spinStart = -1;
  let nextGlint = 2.4;
  let swing = 0;
  let activating = false;
  let activated = false;
  let bumpStart = -1;

  const visible = () =>
    !disposed && contextAvailable && intersecting && document.visibilityState === 'visible';

  function requestRender() {
    if (animationFrame === null && visible()) {
      animationFrame = requestAnimationFrame(render);
    }
  }

  function render(timestamp: number) {
    animationFrame = null;
    if (!visible()) {
      lastFrame = null;
      return;
    }

    if (!reduced && moving && lastFrame !== null) {
      animationTime += Math.min((timestamp - lastFrame) / 1000, 0.05);
    }
    lastFrame = timestamp;
    const time = reduced || !moving ? 0 : animationTime;

    body.update(decorative || options.state === 'running' ? time : 0);
    const step = stepMotion(options.state, time);
    if (reduced || !decorative) {
      pointer.x = 0;
      pointer.y = 0;
      target.x = 0;
      target.y = 0;
      swing = 0;
      spin = 0;
      spinStart = -1;
      rig.scale.setScalar(1);
    } else {
      const previousX = pointer.x;
      pointer.x += (target.x - pointer.x) * 0.06;
      pointer.y += (target.y - pointer.y) * 0.06;
      swing += ((pointer.x - previousX) * 9 - swing) * 0.12;
    }

    head.rotation.y = pointer.x * 0.42;
    head.rotation.x = pointer.y * 0.24;
    body.mesh.rotation.y = -swing * 0.5;
    body.mesh.rotation.z = swing * 0.22;
    rig.position.y = reduced ? 0 : decorative ? Math.sin(time * 1.05) * 0.028 : step.lift;
    rig.rotation.z = reduced || !decorative ? 0 : Math.sin(time * 0.72) * 0.028 - pointer.x * 0.05;

    if (!reduced && bumpStart >= 0 && spinStart < 0) {
      const progress = clamp((animationTime - bumpStart) / 0.65, 0, 1);
      const bump = Math.sin(progress * Math.PI);
      rig.position.y += 0.08 * bump;
      rig.scale.setScalar(1 + 0.06 * bump);
      if (progress >= 1) bumpStart = -1;
    }

    if (spinStart >= 0) {
      const progress = clamp((animationTime - spinStart) / 0.95, 0, 1);
      const eased =
        progress < 0.5
          ? 4 * progress * progress * progress
          : 1 - Math.pow(-2 * progress + 2, 3) / 2;
      spin = eased * Math.PI * 2;
      rig.scale.setScalar(1 + 0.09 * Math.sin(progress * Math.PI));
      if (progress >= 1) {
        spinStart = -1;
        spin = 0;
        rig.scale.setScalar(1);
        if (activating && !activated) {
          activated = true;
          options.onActivate?.();
        }
      }
    }
    const rotationSpeed =
      options.state === 'running' ? Math.PI : options.autoRotate ? Math.PI / 4 : 0;
    rig.rotation.y = reduced ? 0 : time * rotationSpeed + spin + step.rotation;

    shadowMaterial.opacity = 0.46 - rig.position.y * 1.4;
    const shadowScale = 1 - rig.position.y * 0.5;
    shadow.scale.set(0.76 * shadowScale, 0.11 * shadowScale, 1);

    if (!reduced && decorative && time > nextGlint) {
      const progress = (time - nextGlint) / 0.62;
      if (progress >= 1) {
        nextGlint = time + 4.5 + Math.random() * 2.5;
      }
      for (const glint of glints) {
        glint.pivot.rotation.y = glint.base + (progress - 0.5) * 0.34;
        glint.material.opacity = Math.sin(clamp(progress, 0, 1) * Math.PI) * 0.75;
      }
    } else {
      for (const glint of glints) {
        glint.material.opacity = 0;
      }
    }

    renderer.render(scene, camera);
    if (!ready) {
      ready = true;
      options.onReady?.();
    }
    if (!reduced && moving) {
      requestRender();
    }
  }

  function resize() {
    const width = host.clientWidth;
    const height = host.clientHeight;
    if (width === 0 || height === 0) {
      return;
    }
    renderer.setSize(width, height, false);
    camera.aspect = width / height;
    camera.updateProjectionMatrix();
    requestRender();
  }

  function applyTheme() {
    const palette = readPalette(host, options.state);
    bodyMaterial.color.copy(palette.body);
    bodyMaterial.sheenColor.copy(palette.key);
    lensMaterial.color.copy(palette.lens);
    hemisphere.color.copy(palette.sky);
    hemisphere.groundColor.copy(palette.ground);
    key.color.copy(palette.key);
    fill.color.copy(palette.fill);
    rim.color.copy(palette.rim);
    shadowMaterial.color.copy(palette.shadow);
    for (const glint of glints) {
      glint.material.color.copy(palette.glint);
    }
    requestRender();
  }

  const onPointerMove = (event: PointerEvent) => {
    if (reduced || !decorative) {
      return;
    }
    const bounds = host.getBoundingClientRect();
    if (bounds.width === 0 || bounds.height === 0) {
      return;
    }
    target.x = clamp(((event.clientX - bounds.left) / bounds.width - 0.5) * 2, -1.6, 1.6);
    target.y = clamp(((event.clientY - bounds.top) / bounds.height - 0.5) * 2, -1.6, 1.6);
  };
  const onClick = () => {
    if (options.onActivate !== undefined) {
      if (activating || activated) return;
      host.setAttribute('data-activating', '');
      if (reduced) {
        activated = true;
        options.onActivate();
        return;
      }
      activating = true;
      bumpStart = -1;
      spinStart = animationTime;
      return;
    }
    if (!reduced && decorative && spinStart < 0) {
      spinStart = animationTime;
    }
  };
  const onInvite = () => {
    if (options.onActivate === undefined || reduced || activating || activated) return;
    bumpStart = animationTime;
    requestRender();
  };
  const onVisibility = () => {
    lastFrame = null;
    requestRender();
  };
  const onMotion = (event: MediaQueryListEvent) => {
    reduced = event.matches;
    if (reduced && activating && !activated) {
      activated = true;
      options.onActivate?.();
    }
    syncInteraction();
    lastFrame = null;
    requestRender();
  };
  const onContextLost = (event: Event) => {
    event.preventDefault();
    contextAvailable = false;
    syncInteraction();
    if (animationFrame !== null) {
      cancelAnimationFrame(animationFrame);
      animationFrame = null;
    }
    options.onUnavailable?.();
  };

  window.addEventListener('pointermove', onPointerMove, { passive: true });
  window.addEventListener('resize', resize);
  host.addEventListener('click', onClick);
  host.addEventListener('pointerenter', onInvite);
  host.addEventListener('focus', onInvite);
  document.addEventListener('visibilitychange', onVisibility);
  motion.addEventListener('change', onMotion);
  renderer.domElement.addEventListener('webglcontextlost', onContextLost);

  const resizeObserver = new ResizeObserver(resize);
  resizeObserver.observe(host);
  const themeObserver = new MutationObserver(applyTheme);
  themeObserver.observe(document.body, {
    attributes: true,
    attributeFilter: ['data-ds-dark-theme'],
  });
  const intersectionObserver = new IntersectionObserver((entries) => {
    intersecting = entries.some((entry) => entry.isIntersecting);
    lastFrame = null;
    requestRender();
  });
  intersectionObserver.observe(host);

  applyTheme();
  resize();
  requestRender();

  return {
    dispose: () => {
      if (disposed) {
        return;
      }
      disposed = true;
      if (animationFrame !== null) {
        cancelAnimationFrame(animationFrame);
      }
      window.removeEventListener('pointermove', onPointerMove);
      window.removeEventListener('resize', resize);
      host.removeEventListener('click', onClick);
      host.removeEventListener('pointerenter', onInvite);
      host.removeEventListener('focus', onInvite);
      document.removeEventListener('visibilitychange', onVisibility);
      motion.removeEventListener('change', onMotion);
      renderer.domElement.removeEventListener('webglcontextlost', onContextLost);
      resizeObserver.disconnect();
      themeObserver.disconnect();
      intersectionObserver.disconnect();

      const geometries = new Set<THREE.BufferGeometry>();
      const materials = new Set<THREE.Material>();
      scene.traverse((object) => {
        if (object instanceof THREE.Mesh) {
          geometries.add(object.geometry);
          const owned = Array.isArray(object.material) ? object.material : [object.material];
          for (const material of owned) {
            materials.add(material);
          }
        }
        if (object instanceof THREE.Sprite) {
          materials.add(object.material);
        }
      });
      for (const geometry of geometries) {
        geometry.dispose();
      }
      for (const material of materials) {
        material.dispose();
      }
      shadowMap.dispose();
      scene.clear();
      renderer.dispose();
      renderer.forceContextLoss();
      renderer.domElement.remove();
      host.removeAttribute('data-motion');
      host.removeAttribute('data-activating');
      if (host instanceof HTMLButtonElement) {
        host.disabled = true;
      }
    },
  };
}

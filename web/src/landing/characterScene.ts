import * as THREE from 'three';
import { stepMotion } from './characterMotion';
import { readPalette, type CharacterState } from './characterPalette';
import { clamp } from './characterGeometry';
import { createCharacterRig } from '../character/rig';

interface CharacterOptions {
  readonly onReady?: () => void;
  readonly onUnavailable?: () => void;
  readonly state?: CharacterState;
  readonly autoRotate?: boolean;
  readonly onActivate?: () => void;
}

export interface CharacterHandle {
  readonly dispose: () => void;
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

  const character = createCharacterRig();
  character.setShadow(decorative);

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

    const step = stepMotion(options.state, time);
    let scale = 1;
    if (reduced || !decorative) {
      pointer.x = 0;
      pointer.y = 0;
      target.x = 0;
      target.y = 0;
      swing = 0;
      spin = 0;
      spinStart = -1;
    } else {
      const previousX = pointer.x;
      pointer.x += (target.x - pointer.x) * 0.06;
      pointer.y += (target.y - pointer.y) * 0.06;
      swing += ((pointer.x - previousX) * 9 - swing) * 0.12;
    }

    let y = reduced ? 0 : decorative ? Math.sin(time * 1.05) * 0.028 : step.lift;
    const rotZ = reduced || !decorative ? 0 : Math.sin(time * 0.72) * 0.028 - pointer.x * 0.05;

    if (!reduced && bumpStart >= 0 && spinStart < 0) {
      const progress = clamp((animationTime - bumpStart) / 0.65, 0, 1);
      const bump = Math.sin(progress * Math.PI);
      y += 0.08 * bump;
      scale = 1 + 0.06 * bump;
      if (progress >= 1) bumpStart = -1;
    }

    if (spinStart >= 0) {
      const progress = clamp((animationTime - spinStart) / 0.95, 0, 1);
      const eased =
        progress < 0.5
          ? 4 * progress * progress * progress
          : 1 - Math.pow(-2 * progress + 2, 3) / 2;
      spin = eased * Math.PI * 2;
      scale = 1 + 0.09 * Math.sin(progress * Math.PI);
      if (progress >= 1) {
        spinStart = -1;
        spin = 0;
        scale = 1;
        if (activating && !activated) {
          activated = true;
          options.onActivate?.();
        }
      }
    }
    const rotationSpeed =
      options.state === 'running' ? Math.PI : options.autoRotate ? Math.PI / 4 : 0;

    let glint = -1;
    if (!reduced && decorative && time > nextGlint) {
      const progress = (time - nextGlint) / 0.62;
      if (progress >= 1) {
        nextGlint = time + 4.5 + Math.random() * 2.5;
      }
      glint = progress;
    }

    character.applyFrame({
      headYaw: pointer.x * 0.42,
      headPitch: pointer.y * 0.24,
      swing,
      hem: decorative || options.state === 'running' ? time : 0,
      y,
      rotX: 0,
      rotY: reduced ? 0 : time * rotationSpeed + spin + step.rotation,
      rotZ,
      scale: [scale, scale, scale],
      glint,
    });

    renderer.render(character.scene, character.camera);
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
    character.setAspect(width / height);
    requestRender();
  }

  function applyTheme() {
    character.applyPalette(readPalette(host, options.state));
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

      character.dispose();
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

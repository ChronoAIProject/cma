import * as THREE from 'three';
import { readCharacterPalette } from '../landing/characterPalette';
import { createCharacterRig } from './rig';
import { createCharacterStage, type StageEnvironment, type StageSurface } from './stage';

/**
 * The browser's half of the shared stage: one WebGLRenderer whose buffer every character is
 * rendered into and copied out of. Loaded on demand so Three.js never sits in the shell's
 * first bundle.
 */
function createSurface(): StageSurface | undefined {
  if (typeof window.WebGL2RenderingContext === 'undefined') return undefined;
  let renderer: THREE.WebGLRenderer;
  try {
    renderer = new THREE.WebGLRenderer({
      canvas: document.createElement('canvas'),
      antialias: true,
      alpha: true,
      // Each character is copied out right after it renders, before the next one reuses
      // the buffer; keeping it avoids depending on when a browser composites.
      preserveDrawingBuffer: true,
    });
  } catch (error) {
    console.warn('[character] WebGL renderer could not start', error);
    return undefined;
  }
  renderer.setPixelRatio(1);
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.toneMapping = THREE.ACESFilmicToneMapping;
  renderer.setClearColor(0x000000, 0);
  renderer.setScissorTest(true);
  const canvas = renderer.domElement;
  let width = 0;
  let height = 0;
  return {
    source: canvas,
    ensureSize(needWidth, needHeight) {
      if (needWidth > width || needHeight > height) {
        width = Math.max(width, needWidth);
        height = Math.max(height, needHeight);
        renderer.setSize(width, height, false);
      }
      return { width, height };
    },
    draw(rig, drawWidth, drawHeight, exposure) {
      renderer.setViewport(0, 0, drawWidth, drawHeight);
      renderer.setScissor(0, 0, drawWidth, drawHeight);
      renderer.toneMappingExposure = exposure;
      renderer.clear();
      rig.setAspect(drawWidth / drawHeight);
      renderer.render(rig.scene, rig.camera);
    },
    onLost(listener) {
      canvas.addEventListener('webglcontextlost', (event) => {
        event.preventDefault();
        listener();
      });
    },
  };
}

function watchMedia(query: string, listener: () => void) {
  const media = window.matchMedia(query);
  media.addEventListener('change', listener);
  return () => media.removeEventListener('change', listener);
}

export const browserStageEnvironment: StageEnvironment = {
  createSurface,
  createRig: createCharacterRig,
  readPalette: readCharacterPalette,
  requestFrame: (callback) => window.requestAnimationFrame(callback),
  cancelFrame: (handle) => window.cancelAnimationFrame(handle),
  reducedMotion: () => window.matchMedia('(prefers-reduced-motion: reduce)').matches,
  pixelRatio: () => window.devicePixelRatio || 1,
  documentVisible: () => document.visibilityState === 'visible',
  watchReducedMotion: (listener) => watchMedia('(prefers-reduced-motion: reduce)', listener),
  watchTheme(listener) {
    const observer = new MutationObserver(listener);
    observer.observe(document.body, { attributes: true, attributeFilter: ['data-ds-dark-theme'] });
    return () => observer.disconnect();
  },
  watchDocumentVisibility(listener) {
    document.addEventListener('visibilitychange', listener);
    return () => document.removeEventListener('visibilitychange', listener);
  },
  watchVisibility(element, listener) {
    const observer = new IntersectionObserver((entries) => {
      listener(entries.some((entry) => entry.isIntersecting));
    });
    observer.observe(element);
    return () => observer.disconnect();
  },
  watchResize(element, listener) {
    const observer = new ResizeObserver(listener);
    observer.observe(element);
    return () => observer.disconnect();
  },
  log(level, message, detail) {
    const parts = detail === undefined ? [] : [detail];
    if (level === 'error') console.error(`[character] ${message}`, ...parts);
    else console.warn(`[character] ${message}`, ...parts);
  },
};

/** The page's one stage; every CharacterAvatar attaches here. */
export const characterStage = createCharacterStage(browserStageEnvironment);

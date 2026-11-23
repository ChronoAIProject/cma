/*!
 * Copyright (c) 2026 ReactDrops. ReactDrops Component License 1.0.
 * Adapted from crisjavr/reactdrops @ aed0014649a2f0964041ac33c5f02dc3a40c5e13.
 * See LICENSE, NOTICE and source-map.json in this directory.
 */
import { fragment, vertex } from './topographic-surface-shaders';

export function createSurfaceResources(gl: WebGL2RenderingContext) {
  const releases: (() => void)[] = [];
  const dispose = () => {
    for (const release of releases.splice(0).reverse()) {
      try {
        release();
      } catch {
        // A lost context must not prevent the other resources from being released.
      }
    }
  };
  function allocate<T>(resource: T | null, release: (value: T) => void): T {
    if (!resource) throw new Error('render_failed');
    releases.push(() => release(resource));
    return resource;
  }
  try {
    const buffer = allocate(gl.createBuffer(), (value) => gl.deleteBuffer(value));
    const heightTexture = allocate(gl.createTexture(), (value) => gl.deleteTexture(value));
    const program = allocate(gl.createProgram(), (value) => gl.deleteProgram(value));
    for (const [type, source] of [
      [gl.VERTEX_SHADER, vertex],
      [gl.FRAGMENT_SHADER, fragment],
    ] as const) {
      const shader = allocate(gl.createShader(type), (value) => gl.deleteShader(value));
      gl.shaderSource(shader, source);
      gl.compileShader(shader);
      if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) throw new Error('render_failed');
      gl.attachShader(program, shader);
    }
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error('render_failed');
    return {
      buffer,
      heightTexture,
      program,
      uniform: (name: string) => gl.getUniformLocation(program, name),
      dispose,
    };
  } catch (error) {
    dispose();
    throw error;
  }
}

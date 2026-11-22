/*!
 * Copyright (c) 2026 ReactDrops. ReactDrops Component License 1.0.
 * Adapted from crisjavr/reactdrops @ aed0014649a2f0964041ac33c5f02dc3a40c5e13.
 * See LICENSE, NOTICE and source-map.json in this directory.
 */
import { createElevationField } from './topographic-field';
import { createSurfaceResources } from './topographic-surface-resources';

type Viewport = {
  width: number;
  height: number;
  ratio: number;
  pixelsWide: number;
  pixelsHigh: number;
};
type Palette = { line: readonly number[]; accent: readonly number[] };

export function createTopographicSurface(
  canvas: HTMLCanvasElement,
  gl: WebGL2RenderingContext,
  takeFrame: () => { elapsed: number; submit: (draw: () => void) => boolean } | null,
) {
  const resources = createSurfaceResources(gl);
  let disposed = false;
  let width = 0;
  let height = 0;
  const uniforms = new Map<string, WebGLUniformLocation | null>();
  const uniform = (name: string) => {
    if (!uniforms.has(name)) uniforms.set(name, resources.uniform(name));
    return uniforms.get(name) ?? null;
  };
  const rebuild = (viewport: Viewport) => {
    width = viewport.width;
    height = viewport.height;
    const scale = 270,
      distortion = 2.4,
      margin = scale * 0.45;
    const spanX = width + 2 * margin,
      spanY = height + 2 * margin;
    const cell = Math.max(4, Math.sqrt((spanX * spanY) / 60000), Math.max(spanX, spanY) / 510);
    const columns = Math.ceil(spanX / cell) + 3,
      rows = Math.ceil(spanY / cell) + 3;
    const data = new Float32Array(columns * rows * 4),
      sample = createElevationField(42);
    const seed = ((((42 * 0.61803398875) % 1) + 1) % 1) * Math.PI * 2;
    const amount = 0.13 + 0.02 * distortion;
    for (let y = 0; y < rows; y++)
      for (let x = 0; x < columns; x++) {
        const px = ((x / (columns - 1)) * spanX - margin) / scale;
        const py = ((y / (rows - 1)) * spanY - margin) / scale;
        const index = (y * columns + x) * 4;
        data[index] = sample(px, py, 0, distortion);
        data[index + 1] =
          amount * 0.8 * Math.sin(0.85 * px - 0.4 * py + seed) * Math.cos(0.7 * py + seed * 0.6);
        data[index + 2] =
          amount *
          1.15 *
          Math.cos(0.65 * px + 0.8 * py + seed * 0.8) *
          Math.sin(0.9 * py - 0.35 * px + seed + 1.1);
        data[index + 3] = 1;
      }
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, resources.heightTexture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    const internal = gl.getExtension('OES_texture_float_linear') ? gl.RGBA32F : gl.RGBA16F;
    gl.texImage2D(gl.TEXTURE_2D, 0, internal, columns, rows, 0, gl.RGBA, gl.FLOAT, data);
    gl.uniform2f(uniform('uGrid'), columns, rows);
    gl.uniform2f(uniform('uExtent'), spanX, spanY);
    gl.uniform1f(uniform('uMargin'), margin);
  };
  return {
    renderFrame(viewport: Viewport, palette: Palette): boolean {
      if (disposed) return false;
      const grant = takeFrame();
      if (grant === null) return false;
      if (canvas.width !== viewport.pixelsWide) canvas.width = viewport.pixelsWide;
      if (canvas.height !== viewport.pixelsHigh) canvas.height = viewport.pixelsHigh;
      gl.useProgram(resources.program);
      gl.bindBuffer(gl.ARRAY_BUFFER, resources.buffer);
      if (width === 0) {
        gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
        gl.enableVertexAttribArray(0);
        gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
        gl.uniform1i(uniform('uHeight'), 0);
        gl.pixelStorei(gl.UNPACK_ALIGNMENT, 1);
      }
      if (width !== viewport.width || height !== viewport.height) rebuild(viewport);
      const pace = (0.18 + 0.27 * Math.log2(1 + Math.max(width, height) / 320)) / 0.63;
      const phase = grant.elapsed * 0.32 * pace * 0.1;
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.uniform1f(uniform('uCosine'), Math.cos(phase) - 1);
      gl.uniform1f(uniform('uSine'), Math.sin(phase));
      gl.uniform1f(uniform('uRatio'), viewport.ratio);
      gl.uniform3fv(uniform('uLineColor'), palette.line);
      gl.uniform3fv(uniform('uAccentColor'), palette.accent);
      if (gl.getError() !== gl.NO_ERROR) throw new Error('render_failed');
      return grant.submit(() => {
        gl.drawArrays(gl.TRIANGLES, 0, 3);
        if (gl.getError() !== gl.NO_ERROR) throw new Error('render_failed');
      });
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      resources.dispose();
    },
  };
}

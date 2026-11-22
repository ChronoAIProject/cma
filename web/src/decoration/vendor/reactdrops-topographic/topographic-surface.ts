/*!
ReactDrops Component License 1.0

Copyright (c) 2026 ReactDrops

1. Scope and definitions

This license applies to ReactDrops software and associated documentation that
expressly carry this license or are distributed under a LICENSE file containing
it (the "Software"). "Licensor" means the copyright holder granting rights in
the Software, and only to the extent that holder has authority to grant them.
"You" means the individual or entity exercising these rights.

An "End Product" is a website, application, service, or other finished project
whose primary purpose is something other than providing reusable components,
source code, templates, or development assets to others.

2. Permitted uses

Subject to this license, Licensor grants You a worldwide, non-exclusive,
royalty-free license to use, copy, modify, and incorporate the Software into
unlimited personal, internal, and commercial End Products. You may charge for
End Products, subscriptions, development services, and work for clients.

You may distribute the Software as an integrated part of an End Product,
including in browser-delivered JavaScript. You may provide End Product source
code to a client, and share the Software with employees and contractors who
need it to develop or maintain End Products. Those recipients may use the
Software for those purposes subject to this license. This permission does not
authorize them to redistribute the Software as a reusable development asset.

3. Restricted redistribution

Unless You obtain separate written permission from Licensor, You may not sell,
sublicense, publish, share, or otherwise redistribute the Software, or a
substantial portion of it, whether unchanged or modified:

(a) as a standalone component, component library, UI kit, source-code bundle,
    registry, or other collection of reusable development assets;
(b) as part of a theme, template, starter project, or similar product offered
    to multiple recipients for reuse in their own projects; or
(c) through a builder or generation service that offers the Software as
    reusable blocks or source-code exports for others' projects.

These restrictions apply to paid and free redistribution. Renaming, restyling,
repackaging, translating, or modifying the Software does not remove them.
Building separate finished websites or applications for multiple clients is
permitted; offering a reusable template or kit containing the Software is not.

Public source-code hosting by Licensor is not permission to republish the
Software under another license. This license does not override any separate
rights granted directly by Licensor or applicable code-hosting terms.

4. Notices and ownership

Retain this license and applicable copyright and third-party notices in source
copies of the Software and permitted source distributions. No visible credit
or ReactDrops logo is required in an End Product's user interface.

Licensor retains ownership of its Software. You retain rights in Your own
contributions, subject to the license for any incorporated Software. No rights
to trademarks, trade names, or logos are granted by this license.

5. Third-party materials and separate grants

Dependencies, fonts, images, videos, and other third-party materials remain
subject to their respective licenses. Separately identified third-party code
is licensed under its stated terms, and these restrictions do not limit rights
granted by its copyright holders. Retain all required third-party notices.
Demo media are not licensed by this document; obtain Your own permission or
replace them with media You are entitled to use.

This license does not revoke rights granted under other valid licenses. It does
not assert ownership of third-party work or grant rights Licensor lacks.

6. Termination

The permissions granted here continue unless terminated for a material breach.
If You fail to cure a material breach within 30 days after receiving written
notice from Licensor, Your permissions terminate. On termination, cease uses
and distributions requiring this license. Rights under independently granted
licenses and rights that applicable law does not allow to be restricted remain
unaffected. Sections 3 through 7 survive termination.

7. Disclaimer

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE, TITLE, AND NON-INFRINGEMENT. TO THE MAXIMUM EXTENT
PERMITTED BY APPLICABLE LAW, LICENSOR AND CONTRIBUTORS SHALL NOT BE LIABLE FOR
ANY CLAIM, DAMAGES, OR OTHER LIABILITY, WHETHER IN CONTRACT, TORT, OR OTHERWISE,
ARISING FROM, OUT OF, OR IN CONNECTION WITH THE SOFTWARE OR ITS USE.

This license does not limit rights or exceptions that applicable law makes
non-waivable. No other permissions are granted by implication.
*/
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

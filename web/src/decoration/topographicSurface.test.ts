import { describe, expect, it, vi } from 'vitest';
import { createTopographicSurface } from './vendor/reactdrops-topographic/topographic-surface';
import { createElevationField } from './vendor/reactdrops-topographic/topographic-field';

function gpu() {
  const calls: Record<string, ReturnType<typeof vi.fn>> = {};
  const allocated: object[] = [];
  const released: object[] = [];
  const constants = new Map<string, number>([['NO_ERROR', 0]]);
  for (const kind of ['Buffer', 'Texture', 'Program', 'Shader']) {
    calls[`create${kind}`] = vi.fn(() => {
      const value = {};
      allocated.push(value);
      return value;
    });
    calls[`delete${kind}`] = vi.fn((value: object) => released.push(value));
  }
  calls.getShaderParameter = vi.fn(() => true);
  calls.getProgramParameter = vi.fn(() => true);
  calls.getError = vi.fn(() => 0);
  calls.getExtension = vi.fn(() => ({}));
  const gl = new Proxy(
    {},
    {
      get(_target, property: string) {
        if (/^[A-Z_0-9]+$/.test(property)) {
          if (!constants.has(property)) constants.set(property, constants.size);
          return constants.get(property);
        }
        return (calls[property] ??= vi.fn());
      },
    },
  ) as WebGL2RenderingContext;
  return { gl, calls, allocated, released };
}
const viewport = { width: 560, height: 104, ratio: 1.5, pixelsWide: 840, pixelsHigh: 156 };
const grant = () => ({
  elapsed: 1,
  submit: (draw: () => void) => {
    draw();
    return true;
  },
});
const palette = { line: [0.2, 0.2, 0.2], accent: [0.6, 0.8, 0.1] };

describe('flat Flow synchronous resource boundary', () => {
  it('does no uploads or draws without a grant, including pending resize and palette', () => {
    const { gl, calls } = gpu();
    const gate = vi.fn(() => null);
    const surface = createTopographicSurface(document.createElement('canvas'), gl, gate);
    expect(surface.renderFrame(viewport, palette)).toBe(false);
    expect(surface.renderFrame({ ...viewport, width: 100 }, palette)).toBe(false);
    expect(calls.texImage2D).toBeUndefined();
    expect(calls.drawArrays).toBeUndefined();
    expect(calls.bufferData).toBeUndefined();
    expect(gate).toHaveBeenCalledTimes(2);
    surface.dispose();
  });
  it('submits exactly one draw per grant and releases resources exactly once', () => {
    const { gl, calls, allocated, released } = gpu();
    const canvas = document.createElement('canvas');
    const surface = createTopographicSurface(canvas, gl, grant);
    expect(surface.renderFrame(viewport, palette)).toBe(true);
    expect(calls.drawArrays).toHaveBeenCalledTimes(1);
    expect(calls.texImage2D).toHaveBeenCalledTimes(1);
    expect(canvas.width * canvas.height).toBe(131040);
    expect(surface.renderFrame(viewport, palette)).toBe(true);
    expect(calls.texImage2D).toHaveBeenCalledTimes(1);
    expect(calls.drawArrays).toHaveBeenCalledTimes(2);
    expect(surface.renderFrame({ ...viewport, width: 300, pixelsWide: 450 }, palette)).toBe(true);
    expect(calls.texImage2D).toHaveBeenCalledTimes(2);
    surface.dispose();
    surface.dispose();
    expect(new Set(released)).toEqual(new Set(allocated));
    expect(released).toHaveLength(allocated.length);
    expect(surface.renderFrame(viewport, palette)).toBe(false);
    expect(calls.drawArrays).toHaveBeenCalledTimes(3);
  });
  it.each(['createBuffer', 'createTexture', 'createProgram', 'createShader'])(
    'rolls back partial allocation if %s fails',
    (method) => {
      const { gl, calls, allocated, released } = gpu();
      calls[method]!.mockReturnValueOnce(null);
      expect(() => createTopographicSurface(document.createElement('canvas'), gl, grant)).toThrow(
        'render_failed',
      );
      expect(new Set(released)).toEqual(new Set(allocated));
    },
  );
  it.each(['getShaderParameter', 'getProgramParameter'])(
    'does not expose driver diagnostics when %s fails',
    (method) => {
      const { gl, calls, allocated, released } = gpu();
      calls[method]!.mockReturnValue(false);
      expect(() => createTopographicSurface(document.createElement('canvas'), gl, grant)).toThrow(
        /^render_failed$/,
      );
      expect(calls.getShaderInfoLog).toBeUndefined();
      expect(calls.getProgramInfoLog).toBeUndefined();
      expect(new Set(released)).toEqual(new Set(allocated));
    },
  );
  it('detects real GL error codes before and after submission', () => {
    const { gl, calls } = gpu();
    const surface = createTopographicSurface(document.createElement('canvas'), gl, grant);
    calls.getError!.mockReturnValueOnce(1285);
    expect(() => surface.renderFrame(viewport, palette)).toThrow('render_failed');
    expect(calls.drawArrays).toBeUndefined();
    calls.getError!.mockReturnValueOnce(0).mockReturnValueOnce(1282);
    expect(() => surface.renderFrame(viewport, palette)).toThrow('render_failed');
    expect(calls.drawArrays).toHaveBeenCalledOnce();
    surface.dispose();
  });
  it('continues releasing all resources when one driver deletion throws', () => {
    const { gl, calls, allocated, released } = gpu();
    const surface = createTopographicSurface(document.createElement('canvas'), gl, grant);
    calls.deleteShader!.mockImplementationOnce((value: object) => {
      released.push(value);
      throw new Error('synthetic');
    });
    expect(() => surface.dispose()).not.toThrow();
    expect(new Set(released)).toEqual(new Set(allocated));
  });
  it('uses half-float storage when float-linear filtering is absent', () => {
    const { gl, calls } = gpu();
    calls.getExtension!.mockReturnValue(null);
    const surface = createTopographicSurface(document.createElement('canvas'), gl, grant);
    surface.renderFrame(viewport, palette);
    expect(calls.texImage2D!.mock.calls[0]![2]).toBe(gl.RGBA16F);
    surface.dispose();
  });
});

it('keeps pinned elevation deterministic, finite and continuous across sample times', () => {
  const first = createElevationField(42),
    second = createElevationField(42);
  const other = createElevationField(7);
  for (const x of [-10, -0.1, 0, 0.1, 10])
    for (const y of [-10, -0.1, 0, 0.1, 10]) {
      const sample = first(x, y, 0, 2.4);
      expect(sample).toBe(second(x, y, 0, 2.4));
      expect(Number.isFinite(sample)).toBe(true);
      expect(Math.abs(first(x + 1e-6, y, 0, 2.4) - sample)).toBeLessThan(0.001);
      expect(Math.abs(first(x, y, 1e-6, 2.4) - sample)).toBeLessThan(0.001);
      expect(other(x, y, 0, 2.4)).not.toBe(sample);
    }
});

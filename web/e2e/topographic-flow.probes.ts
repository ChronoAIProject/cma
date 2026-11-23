import type { Page } from '@playwright/test';

export type FlowProbe = {
  draws: { at: number; width: number; height: number }[];
  contexts: number;
  contextTimes: number[];
  contentReady: boolean[];
  allocations: number;
  releases: number;
  contextReleases: number;
  canvas2d: number;
  pendingFrames: number[];
  fatal: string[];
  errors: string[];
  mascotDraws: number;
};
type ProbeOptions = {
  noGL?: boolean;
  shaderFailure?: boolean;
  allocationFailure?: boolean;
  drawFailure?: boolean;
};
declare global {
  interface Window {
    __flowProbe: FlowProbe;
    __flowLose: () => void;
    __flowRestore: () => void;
  }
}

export async function installFlowProbe(page: Page, options: ProbeOptions = {}) {
  await page.addInitScript((faults) => {
    const state: FlowProbe = (window.__flowProbe = {
      draws: [],
      contexts: 0,
      contextTimes: [],
      contentReady: [],
      allocations: 0,
      releases: 0,
      contextReleases: 0,
      canvas2d: 0,
      pendingFrames: [],
      fatal: [],
      errors: [],
      mascotDraws: 0,
    });
    addEventListener('error', () => state.errors.push('error'));
    addEventListener('unhandledrejection', () => state.errors.push('unhandledrejection'));
    let reporter: ((category: string) => void) | undefined;
    Object.defineProperty(window, 'cmaReportFatal', {
      configurable: true,
      get: () => (category: string) => {
        state.fatal.push(category);
        reporter?.(category);
      },
      set: (value: ((category: string) => void) | undefined) => {
        reporter = value;
      },
    });
    const request = requestAnimationFrame,
      cancel = cancelAnimationFrame;
    let executing = 0;
    const owned = new Set<number>(),
      pending = new Set<number>();
    const publish = () => {
      state.pendingFrames = [...pending];
    };
    window.requestAnimationFrame = (callback) => {
      const parentOwned = owned.has(executing);
      const id = request((time) => {
        pending.delete(id);
        publish();
        const before = executing;
        executing = id;
        try {
          callback(time);
        } finally {
          executing = before;
        }
      });
      if (parentOwned) {
        owned.add(id);
        pending.add(id);
        publish();
      }
      return id;
    };
    window.cancelAnimationFrame = (id) => {
      pending.delete(id);
      publish();
      cancel(id);
    };
    const getContext = HTMLCanvasElement.prototype.getContext;
    const seen = new WeakSet<WebGL2RenderingContext>();
    HTMLCanvasElement.prototype.getContext = function (
      this: HTMLCanvasElement,
      kind: string,
      ...args: unknown[]
    ) {
      const flow = this.closest('.topographic-backdrop') !== null;
      if (flow && kind === '2d') state.canvas2d++;
      if (flow && faults.noGL && kind === 'webgl2') return null;
      const context = Reflect.apply(getContext, this, [kind, ...args]);
      if (kind !== 'webgl2' || !context || seen.has(context)) return context;
      const gl = context as WebGL2RenderingContext;
      seen.add(gl);
      const canvas = gl.canvas as HTMLCanvasElement;
      if (!flow) {
        const draw = gl.drawArrays.bind(gl),
          indexed = gl.drawElements.bind(gl);
        gl.drawArrays = (...params) => {
          state.mascotDraws++;
          return draw(...params);
        };
        gl.drawElements = (...params) => {
          state.mascotDraws++;
          return indexed(...params);
        };
        return gl;
      }
      state.contexts++;
      state.contextTimes.push(performance.now());
      state.contentReady.push(
        !!document.querySelector(
          '.sign-in-landing__actions button:not(:disabled), .task-home__composer textarea',
        ),
      );
      const resources = new Set<object>();
      for (const kind of ['Buffer', 'Texture', 'Program', 'Shader'] as const) {
        const createName = `create${kind}`,
          deleteName = `delete${kind}`;
        const target = gl as unknown as Record<string, (...args: unknown[]) => unknown>;
        const create = target[createName]!.bind(gl),
          remove = target[deleteName]!.bind(gl);
        target[createName] = (...params) => {
          if (faults.allocationFailure && kind === 'Texture') return null;
          const value = create(...params);
          if (value && typeof value === 'object') {
            resources.add(value);
            state.allocations++;
          }
          return value;
        };
        target[deleteName] = (...params) => {
          if (resources.delete(params[0] as object)) state.releases++;
          return remove(...params);
        };
      }
      const shaderParameter = gl.getShaderParameter.bind(gl);
      gl.getShaderParameter = (shader, name) =>
        faults.shaderFailure && name === gl.COMPILE_STATUS ? false : shaderParameter(shader, name);
      const draw = gl.drawArrays.bind(gl);
      gl.drawArrays = (...params) => {
        if (faults.drawFailure) throw new Error('synthetic decoration draw failure');
        if (executing) owned.add(executing);
        state.draws.push({ at: performance.now(), width: canvas.width, height: canvas.height });
        draw(...params);
      };
      const getExtension = gl.getExtension.bind(gl);
      const loss = getExtension('WEBGL_lose_context') as WEBGL_lose_context | null;
      window.__flowLose = () => loss?.loseContext();
      window.__flowRestore = () => loss?.restoreContext();
      gl.getExtension = ((name: string) => {
        if (name !== 'WEBGL_lose_context' || !loss) return getExtension(name);
        return {
          loseContext: () => {
            state.contextReleases++;
            loss.loseContext();
          },
          restoreContext: () => loss.restoreContext(),
        };
      }) as typeof gl.getExtension;
      return gl;
    } as typeof getContext;
  }, options);
}

export function flowProbe(page: Page): Promise<FlowProbe> {
  return page.evaluate(() => window.__flowProbe);
}

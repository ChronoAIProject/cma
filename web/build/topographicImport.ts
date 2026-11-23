import { resolve } from 'node:path';
import { realpathSync } from 'node:fs';
import { normalizePath, type Plugin } from 'vite';

/** Optional decoration failures must reach their local catch before application fatal reporting. */
export function optionalTopographicImport(): Plugin {
  let root = '';
  let enabled = false;
  return {
    name: 'cma-optional-topographic-import',
    apply: 'build',
    enforce: 'pre',
    configResolved(config) {
      root = realpathSync(config.root);
      enabled = config.env.VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED === 'true';
    },
    transform(code, id) {
      if (!enabled || id !== normalizePath(resolve(root, 'src/decoration/topographicLoader.ts')))
        return;
      const source = "import('./vendor/reactdrops-topographic/topographic-surface')";
      if (code.split(source).length !== 2)
        return this.error('Update the optional Flow import boundary before building.');
      const reference = this.emitFile({
        type: 'chunk',
        preserveSignature: 'strict',
        id: resolve(root, 'src/decoration/vendor/reactdrops-topographic/topographic-surface.ts'),
        name: 'topographic-surface',
      });
      // Rollup resolves this native import after Vite's preload-error wrapper pass.
      return {
        code: code.replace(
          source,
          `import(/* @vite-ignore */ import.meta.ROLLUP_FILE_URL_${reference})`,
        ),
        map: null,
      };
    },
  };
}

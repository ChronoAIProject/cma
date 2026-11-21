import type { Plugin } from 'postcss';

/** Scale text declarations and font aliases without scaling layout geometry
 * or multiplying inherited text sizes a second time. */
export function systemText(): Plugin {
  const scale = (value: string) => `calc(${value} * var(--cma-system-font-scale, 1))`;
  const scaleLengths = (value: string) => value.replace(/\b\d+(?:\.\d+)?(?:px|rem)\b/g, scale);
  return {
    postcssPlugin: 'cma-system-text',
    OnceExit(root) {
      root.walkDecls((declaration) => {
        if (declaration.parent?.type === 'atrule') return;
        const { prop, value } = declaration;
        if (value.includes('--cma-system-font-scale')) return;
        if (/^--dsw-alias-[\w-]+-font$/.test(prop)) {
          // Unitless leading follows the scaled font size; lengths need their own scale.
          declaration.value = scaleLengths(value);
        } else if (prop === 'font') {
          const token = /^var\((--dsw-font-[\w-]+)\)$/.exec(value)?.[1];
          if (token) {
            declaration.cloneAfter({
              prop: 'line-height',
              value: scale(`var(${token}-line-height)`),
            });
            declaration.cloneAfter({ prop: 'font-size', value: scale(`var(${token}-font-size)`) });
          } else if (/\d(?:px|rem)\b/.test(value)) {
            declaration.value = scaleLengths(value);
          }
        } else if (
          prop === 'font-size' &&
          !/^(?:inherit|initial|unset|revert|revert-layer)$/.test(value)
        ) {
          // Relative em/% already follows its scaled parent. rem follows the
          // unchanged root and therefore needs the explicit preference.
          if (!/^[\d.]+(?:em|%)$/.test(value)) declaration.value = scale(value);
        } else if (prop === 'line-height' && /\d(?:px|rem)\b/.test(value)) {
          declaration.value = scale(value);
        }
      });
    },
  };
}

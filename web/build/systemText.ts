import type { Plugin } from 'postcss';

/** Apply the user's text preference at the consuming element, so conversation
 * text can opt out without duplicating the shared typography token ramp. */
export function systemText(): Plugin {
  const scale = (value: string) => `calc(${value} * var(--cma-system-font-scale, 1))`;
  return {
    postcssPlugin: 'cma-system-text',
    OnceExit(root) {
      root.walkDecls((declaration) => {
        if (declaration.parent?.type === 'atrule') return;
        const { prop, value } = declaration;
        if (value.includes('--cma-system-font-scale')) return;
        if (prop === 'font') {
          const token = /^var\((--dsw-font-[\w-]+)\)$/.exec(value)?.[1];
          if (token) {
            declaration.cloneAfter({
              prop: 'line-height',
              value: scale(`var(${token}-line-height)`),
            });
            declaration.cloneAfter({ prop: 'font-size', value: scale(`var(${token}-font-size)`) });
          } else if (/\d(?:px|rem)\b/.test(value)) {
            declaration.value = value.replace(/\b\d+(?:\.\d+)?(?:px|rem)\b/g, scale);
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

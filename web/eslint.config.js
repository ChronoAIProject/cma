import js from '@eslint/js';
import globals from 'globals';
import tseslint from 'typescript-eslint';
import reactHooks from 'eslint-plugin-react-hooks';

export default tseslint.config(
  { ignores: ['dist', 'node_modules', 'coverage'] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    /*
     * Build-time scripts run in node, and the callbacks they hand to a browser
     * run in a browser — `page.evaluate` bodies are serialised and executed
     * there, so both sets of globals are genuinely in scope in one file.
     */
    files: ['scripts/**/*.mjs'],
    languageOptions: {
      ecmaVersion: 2022,
      globals: { ...globals.node, ...globals.browser },
    },
  },
  {
    files: ['**/*.{ts,tsx}'],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
    },
    plugins: { 'react-hooks': reactHooks },
    rules: {
      ...reactHooks.configs.recommended.rules,
      /*
       * Terminal hardening is enforced here, not just documented.
       *
       * src/terminal/hardening.ts neutralises these sequences defensively, but
       * xterm's parser runs the most recently registered handler first — so an
       * addon loaded after hardening would win. Banning the imports outright is
       * the control that actually holds. See issue #18 and Web_Style.md §8.
       */
      'no-restricted-imports': [
        'error',
        {
          paths: [
            {
              name: '@xterm/addon-clipboard',
              message:
                'Implements OSC 52 in both directions; the read form pipes the host clipboard back to the agent. Banned by issue #18.',
            },
            {
              name: '@xterm/addon-web-links',
              message:
                'Opens links with no confirmation and matches bare URLs in agent output. Banned by issue #18.',
            },
            {
              name: '@xterm/addon-progress',
              message: 'Registers OSC 9, which src/terminal/hardening.ts neutralises.',
            },
          ],
        },
      ],
    },
  },
);

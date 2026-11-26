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
    files: ['scripts/**/*.mjs', 'e2e/owner-timing/**/*.mjs'],
    languageOptions: {
      ecmaVersion: 2022,
      globals: { ...globals.node, ...globals.browser },
    },
  },
  {
    files: ['public/browser-telemetry.js'],
    languageOptions: { ecmaVersion: 2022, globals: globals.browser },
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
    },
  },
);

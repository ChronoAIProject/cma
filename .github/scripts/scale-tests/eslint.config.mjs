import js from '@eslint/js';
import globals from 'globals';

export default [
  { ignores: ['results/**', 'node_modules/**', 'coverage/**'] },
  js.configs.recommended,
  { files: ['**/*.mjs'], languageOptions: { ecmaVersion: 'latest', sourceType: 'module', globals: globals.node },
    rules: { 'no-unused-vars': ['error', { argsIgnorePattern: '^_' }], 'no-empty': 'error' } },
  { files: ['timing/**/*.mjs'], languageOptions: { globals: { ...globals.node, ...globals.browser } } },
  { files: ['staging/browser.mjs', 'staging/browser.test.mjs'], languageOptions: { globals: { ...globals.node, ...globals.browser } } },
];

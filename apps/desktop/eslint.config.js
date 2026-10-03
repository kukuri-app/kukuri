import js from '@eslint/js';
import globals from 'globals';
import reactHooks from 'eslint-plugin-react-hooks';
import reactRefresh from 'eslint-plugin-react-refresh';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  {
    ignores: [
      'dist',
      // 告知素材の撮影用 preview の build 出力 (#1038)
      'dist-promo',
      // Web の build の出力と、その入力の wasm-bindgen の出力 (#1220)
      'dist-web',
      'web-runtime-pkg',
      'storybook-static',
      'playwright-report',
      'test-results',
      'src-tauri/target',
      'src-tauri/gen',
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ['**/*.{ts,tsx}'],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    plugins: {
      'react-hooks': reactHooks,
      'react-refresh': reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      'react-hooks/set-state-in-effect': 'off',
      'react-refresh/only-export-components': ['warn', { allowConstantExport: true }],
      // AGENTS.md: console.error は使わない。log/debug も禁止し、
      // 意図的なログは warn / info のみ許可する。
      'no-console': ['error', { allow: ['warn', 'info'] }],
    },
  },
  {
    files: ['scripts/**/*.mjs'],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.node,
    },
  },
  {
    // Web の実ブラウザの試験 (#1220)。driver は node、`browser.execute` の関数はページで動く。
    files: ['tests/web-e2e/**/*.mjs'],
    languageOptions: {
      ecmaVersion: 2022,
      globals: { ...globals.node, ...globals.browser },
    },
  },
  {
    files: ['playwright.config.ts', '.storybook/**/*.{ts,tsx}', 'tests/playwright/**/*.ts'],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.node,
    },
  }
);

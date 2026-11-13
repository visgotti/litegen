// ESLint 9 flat config (replaces the legacy .eslintrc.cjs, which ESLint 9 no
// longer reads): typescript-eslint's recommended rules, with Prettier owning
// formatting.
import tseslint from '@typescript-eslint/eslint-plugin';
import prettier from 'eslint-config-prettier';

export default [
  { ignores: ['dist/**', 'node_modules/**', 'coverage/**', 'coverage-e2e/**'] },
  ...tseslint.configs['flat/recommended'],
  prettier,
  {
    rules: {
      '@typescript-eslint/explicit-function-return-type': 'off',
      '@typescript-eslint/explicit-module-boundary-types': 'off',
      '@typescript-eslint/no-explicit-any': 'off',
    },
  },
];

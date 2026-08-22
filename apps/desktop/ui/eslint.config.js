import js from '@eslint/js';
import tseslint from 'typescript-eslint';
import reactHooks from 'eslint-plugin-react-hooks';

/**
 * Deliberately narrow.
 *
 * This exists for one rule. A hook added after an early return blanked the
 * entire app — React threw #310 before rendering anything — and neither
 * TypeScript nor the tests could see it, because it is not a type error and
 * the component never got far enough to be tested. It took a real window and a
 * log file to find.
 *
 * Style rules are left off on purpose: a lint run that reports fifty opinions
 * is one people stop reading, and then it stops catching the one thing that
 * actually breaks the app.
 */
export default tseslint.config(
  { ignores: ['dist/**', 'node_modules/**'] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ['src/**/*.{ts,tsx}'],
    plugins: { 'react-hooks': reactHooks },
    rules: {
      'react-hooks/rules-of-hooks': 'error',
      // A warning, not an error: the dependency rule is right most of the time
      // and wrong often enough that making it fatal teaches people to silence
      // it, which loses the cases where it was right.
      'react-hooks/exhaustive-deps': 'warn',

      // Off — TypeScript already reports these, with better messages.
      'no-undef': 'off',
      'no-unused-vars': 'off',
      '@typescript-eslint/no-unused-vars': [
        'error',
        { argsIgnorePattern: '^_', varsIgnorePattern: '^_' },
      ],
      // Empty catch blocks are used deliberately here: several failures are
      // genuinely nothing-to-do-about-it, and each one carries a comment
      // saying so.
      'no-empty': ['error', { allowEmptyCatch: true }],
    },
  },
);

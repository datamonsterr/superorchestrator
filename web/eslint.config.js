import js from '@eslint/js'
import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import reactRefresh from 'eslint-plugin-react-refresh'
import tseslint from 'typescript-eslint'

export default tseslint.config(
  { ignores: ['dist', 'node_modules', 'public'] },
  {
    files: ['**/*.{ts,tsx}'],
    extends: [
      js.configs.recommended,
      tseslint.configs.recommended,
      reactHooks.configs.flat.recommended,
      reactRefresh.configs.vite,
    ],
    languageOptions: { globals: globals.browser },
  },
  {
    // shadcn-generated files export variants alongside components and use the stock mobile hook.
    files: ['src/components/ui/**', 'src/hooks/**'],
    rules: { 'react-refresh/only-export-components': 'off', 'react-hooks/set-state-in-effect': 'off' },
  },
)

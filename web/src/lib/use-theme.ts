import { createContext, useContext } from 'react'
import type { Theme } from './theme'

export interface ThemeContextValue {
  theme: Theme
  resolved: 'light' | 'dark'
  setTheme: (t: Theme) => void
}
export const ThemeContext = createContext<ThemeContextValue>({ theme: 'system', resolved: 'light', setTheme: () => {} })
export const useTheme = () => useContext(ThemeContext)

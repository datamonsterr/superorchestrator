import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react'
import { applyTheme, readStoredTheme, resolveTheme, storeTheme, type Theme } from '@/lib/theme'
import { ThemeContext } from '@/lib/use-theme'

/** Class strategy on <html>; follows the OS when set to "system". */
export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(readStoredTheme)
  const [resolved, setResolved] = useState(() => resolveTheme(theme))

  useEffect(() => {
    const sync = () => {
      applyTheme(theme)
      setResolved(resolveTheme(theme))
    }
    sync()
    if (theme !== 'system' || typeof window.matchMedia !== 'function') return
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    mq.addEventListener('change', sync)
    return () => mq.removeEventListener('change', sync)
  }, [theme])

  const setTheme = useCallback((t: Theme) => {
    storeTheme(t)
    setThemeState(t)
  }, [])

  const value = useMemo(() => ({ theme, resolved, setTheme }), [theme, resolved, setTheme])
  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
}

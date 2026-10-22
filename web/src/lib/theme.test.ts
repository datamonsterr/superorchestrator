import { beforeEach, describe, expect, it, vi } from 'vitest'
import { THEME_KEY, applyTheme, readStoredTheme, resolveTheme, storeTheme } from './theme'

beforeEach(() => {
  localStorage.clear()
  document.documentElement.classList.remove('dark')
})

describe('theme storage', () => {
  it('defaults to system and round-trips', () => {
    expect(readStoredTheme()).toBe('system')
    storeTheme('dark')
    expect(readStoredTheme()).toBe('dark')
    localStorage.setItem(THEME_KEY, 'bogus')
    expect(readStoredTheme()).toBe('system')
  })

  it('survives blocked storage', () => {
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => {
      throw new Error('blocked')
    })
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('blocked')
    })
    expect(readStoredTheme()).toBe('system')
    expect(() => storeTheme('light')).not.toThrow()
    vi.restoreAllMocks()
  })
})

describe('applyTheme', () => {
  it('sets the dark class and color-scheme', () => {
    applyTheme('dark')
    expect(document.documentElement).toHaveClass('dark')
    expect(document.documentElement.style.colorScheme).toBe('dark')
    applyTheme('light')
    expect(document.documentElement).not.toHaveClass('dark')
  })
  it('resolves system from matchMedia', () => {
    vi.spyOn(window, 'matchMedia').mockReturnValue({ matches: true } as MediaQueryList)
    expect(resolveTheme('system')).toBe('dark')
    vi.restoreAllMocks()
  })
})

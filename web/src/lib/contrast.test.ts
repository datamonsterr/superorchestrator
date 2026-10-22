import { describe, expect, it } from 'vitest'
import { contrastRatio, luminance, parseHex, readableInk } from './contrast'

describe('contrast', () => {
  it('parses short and long hex', () => {
    expect(parseHex('#fff')).toEqual([255, 255, 255])
    expect(parseHex('0a141e')).toEqual([10, 20, 30])
    expect(() => parseHex('nope')).toThrow()
  })
  it('matches the WCAG reference values', () => {
    expect(luminance('#000000')).toBe(0)
    expect(luminance('#ffffff')).toBe(1)
    expect(contrastRatio('#000000', '#ffffff')).toBeCloseTo(21, 5)
    expect(contrastRatio('#777777', '#ffffff')).toBeCloseTo(4.48, 1)
    expect(contrastRatio('#fff', '#fff')).toBe(1)
  })
  it('picks the readable ink', () => {
    expect(readableInk('#000000')).toBe('#ffffff')
    expect(readableInk('#fde68a')).toBe('#0a0a0a')
  })
})

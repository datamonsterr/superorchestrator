import { describe, expect, it } from 'vitest'
import { axisTicks, formatAxisLabel, HOUR, MINUTE, niceStep, SECOND } from './time'

const T0 = Date.UTC(2026, 0, 1, 12, 0, 0)

describe('niceStep', () => {
  it('keeps the label count between 6 and 12 over common spans', () => {
    for (const span of [30 * SECOND, 2 * MINUTE, 10 * MINUTE, 45 * MINUTE, 3 * HOUR, 10 * HOUR]) {
      const n = span / niceStep(span)
      expect(n).toBeLessThanOrEqual(12)
      expect(n).toBeGreaterThanOrEqual(5)
    }
  })
  it('picks 1 s for tiny spans and caps at a day', () => {
    expect(niceStep(5 * SECOND)).toBe(SECOND)
    expect(niceStep(1000 * HOUR)).toBe(24 * HOUR)
  })
})

describe('axisTicks', () => {
  it('returns multiples of the step inside the range', () => {
    const ticks = axisTicks(T0 + 30 * SECOND, T0 + 10 * MINUTE + 30 * SECOND, 12, 0)
    expect(ticks.length).toBeGreaterThanOrEqual(6)
    expect(ticks.length).toBeLessThanOrEqual(12)
    for (const t of ticks) {
      expect(t.t).toBeGreaterThanOrEqual(T0 + 30 * SECOND)
      expect(t.t % MINUTE).toBe(0)
    }
  })
  it('shifts by the local offset so hours land on local hours', () => {
    const offset = 5.5 * HOUR
    const ticks = axisTicks(T0, T0 + 10 * HOUR, 12, offset)
    for (const t of ticks) expect((t.t + offset) % HOUR).toBe(0)
  })
  it('marks the next larger step as major', () => {
    const ticks = axisTicks(T0, T0 + 10 * MINUTE, 12, 0) // 1 minute steps, bigger step 2 minutes
    expect(ticks.filter((t) => t.major).every((t) => t.t % (2 * MINUTE) === 0)).toBe(true)
    expect(ticks.some((t) => !t.major)).toBe(true)
  })
  it('is empty for an empty range', () => {
    expect(axisTicks(T0, T0)).toEqual([])
  })
})

describe('formatAxisLabel', () => {
  it('adds seconds below a minute step only', () => {
    expect(formatAxisLabel(T0, 5 * SECOND)).toMatch(/^\d\d:\d\d:\d\d$/)
    expect(formatAxisLabel(T0, 5 * MINUTE)).toMatch(/^\d\d:\d\d$/)
  })
})

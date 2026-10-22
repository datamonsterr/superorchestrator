import { describe, expect, it } from 'vitest'
import { contrastRatio, readableInk } from '@/lib/contrast'
import { stateCss, LANE_BG, LEGEND, MARKER_ORDER, MARKER_STYLES, STATE_ORDER, STATE_STYLES, segmentInk, stateVarName } from './colors'

describe('state colours', () => {
  for (const theme of ['light', 'dark'] as const) {
    it(`segment fills reach 3:1 against the ${theme} lane background`, () => {
      for (const s of STATE_ORDER) {
        const ratio = contrastRatio(STATE_STYLES[s][theme], LANE_BG[theme])
        expect(ratio, `${s} (${theme}) ${ratio.toFixed(2)}`).toBeGreaterThanOrEqual(3)
      }
    })
    it(`segment text reaches 4.5:1 in the ${theme} theme`, () => {
      for (const s of STATE_ORDER) {
        const ratio = contrastRatio(segmentInk(s, theme), STATE_STYLES[s][theme])
        expect(ratio, `${s} (${theme}) ${ratio.toFixed(2)}`).toBeGreaterThanOrEqual(4.5)
        expect(segmentInk(s, theme)).toBe(readableInk(STATE_STYLES[s][theme]))
      }
    })
  }

  it('gives every state a cue besides colour: a pattern or an end glyph', () => {
    const cues = STATE_ORDER.map((s) => `${STATE_STYLES[s].pattern}${STATE_STYLES[s].endGlyph ?? ''}`)
    expect(new Set(cues).size).toBe(cues.length - 1) // escalated shares cross hatch with stalled and adds a marker
    for (const s of STATE_ORDER) expect(STATE_STYLES[s].patternLabel).not.toBe('')
  })

  it('emits every colour and ink as a CSS variable for both themes', () => {
    const STATE_CSS = stateCss()
    const [light, dark] = STATE_CSS.split('.dark')
    expect(light).toContain(':root {')
    for (const s of STATE_ORDER) {
      const v = stateVarName(s)
      expect(light, v).toContain(`${v}: ${STATE_STYLES[s].light};`)
      expect(dark, v).toContain(`${v}: ${STATE_STYLES[s].dark};`)
      expect(light, v).toContain(`${v}-ink: ${segmentInk(s, 'light')};`)
      expect(dark, v).toContain(`${v}-ink: ${segmentInk(s, 'dark')};`)
    }
    expect(STATE_CSS).toContain('--state-pending-review:')
  })

  it('lists every state and marker glyph, including join, in the legend', () => {
    expect(LEGEND.states.map((s) => s.state)).toEqual(STATE_ORDER)
    expect(LEGEND.markers.map((m) => m.kind)).toEqual(MARKER_ORDER)
    expect(LEGEND.markers.map((m) => m.kind)).toContain('join')
    for (const k of MARKER_ORDER) expect(MARKER_STYLES[k].label).not.toBe('')
  })
})

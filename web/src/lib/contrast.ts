/** WCAG 2.x contrast helpers (test use and dev assertions). Inputs are `#rgb` or `#rrggbb`. */

export function parseHex(hex: string): [number, number, number] {
  let h = hex.trim().replace(/^#/, '')
  if (h.length === 3) h = [...h].map((c) => c + c).join('')
  if (!/^[0-9a-f]{6}$/i.test(h)) throw new Error(`invalid hex colour: ${hex}`)
  return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)) as [number, number, number]
}

/** Relative luminance, 0 (black) to 1 (white). */
export function luminance(hex: string): number {
  const [r, g, b] = parseHex(hex).map((v) => {
    const c = v / 255
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

/** Contrast ratio between two colours, 1 to 21. */
export function contrastRatio(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return (hi + 0.05) / (lo + 0.05)
}

export const INK_LIGHT = '#ffffff'
export const INK_DARK = '#0a0a0a'

/** White or near-black, whichever reads better on `fill`. */
export function readableInk(fill: string): string {
  return contrastRatio(fill, INK_LIGHT) >= contrastRatio(fill, INK_DARK) ? INK_LIGHT : INK_DARK
}

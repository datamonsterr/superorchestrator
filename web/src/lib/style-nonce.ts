/** Each HTML response supplies a nonce for trusted runtime style elements. */
export function styleNonce(): string | undefined {
  return document.querySelector<HTMLMetaElement>('meta[name="so-style-nonce"]')?.content
}

declare global {
  interface Window { __webpack_nonce__?: string }
}

// react-style-singleton (Radix scroll locking) reads this conventional global.
window.__webpack_nonce__ = styleNonce()

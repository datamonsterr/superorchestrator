// Classic script (CSP-safe, no inline): applies the stored theme before first paint.
// Keep in sync with src/lib/theme.ts.
;(function () {
  try {
    var t = localStorage.getItem('so-theme')
    var dark = t === 'dark' || ((t !== 'light') && window.matchMedia('(prefers-color-scheme: dark)').matches)
    var r = document.documentElement
    r.classList.toggle('dark', dark)
    r.style.colorScheme = dark ? 'dark' : 'light'
  } catch (e) {}
})()

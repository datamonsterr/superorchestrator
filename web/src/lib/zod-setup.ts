import { z } from 'zod'

// The server's CSP has no 'unsafe-eval': without this zod probes `new Function` and Chromium logs a violation.
// Schema modules import this file for the side effect, so zod only loads with the pages that use it.
z.config({ jitless: true })

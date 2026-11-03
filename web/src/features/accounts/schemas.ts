import '@/lib/zod-setup'
import { z } from 'zod'
import type { AddAccountRequest } from '@/api/types'
import { ENDPOINT_PRESETS } from './endpoint-presets'

export const HARNESSES = ['codex', 'agy', 'opencode', 'claude'] as const
export const AGY_POOLS = ['gemini', 'claude'] as const


const ENV_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/

function isAllowedBaseUrl(raw: string): boolean {
  try {
    const u = new URL(raw)
    if (u.protocol === 'https:') return true
    return u.protocol === 'http:' && (u.hostname === 'localhost' || u.hostname === '127.0.0.1')
  } catch {
    return false
  }
}

/** Account names double as directory and wrapper names (design 5). */
export function addAccountSchema(existing: readonly string[]) {
  return z
    .object({
    harness: z.enum(HARNESSES),
    name: z
      .string()
      .min(1, 'Enter a name')
      .regex(/^[a-z0-9-]+$/, 'Use lowercase letters, digits and dashes')
      .refine((n) => !existing.includes(n), 'An account with this name already exists'),
    home: z.string().trim().optional(),
    endpointPreset: z.enum(ENDPOINT_PRESETS).optional(),
    baseUrl: z.string().trim().optional(),
    models: z.string().optional(),
    slotOpus: z.string().trim().optional(),
    slotSonnet: z.string().trim().optional(),
    slotHaiku: z.string().trim().optional(),
    tokenSource: z.enum(['paste', 'env']).optional(),
    token: z.string().trim().optional(),
    tokenEnv: z.string().trim().optional(),
  })
    .superRefine((v, ctx) => {
      const preset = v.endpointPreset ?? 'none'
      if (v.harness !== 'claude' || preset === 'none') return
      const need = (path: string, message: string) => ctx.addIssue({ code: 'custom', path: [path], message })
      if (preset === 'custom') {
        if (!v.baseUrl) need('baseUrl', 'Enter the base URL')
        else if (!isAllowedBaseUrl(v.baseUrl)) need('baseUrl', 'Use https, or http on localhost or 127.0.0.1')
        if (parseModels(v.models).length === 0) need('models', 'Enter at least one model')
        if (!v.slotOpus) need('slotOpus', 'Enter the opus model')
        if (!v.slotSonnet) need('slotSonnet', 'Enter the sonnet model')
        if (!v.slotHaiku) need('slotHaiku', 'Enter the haiku model')
      }
      if (v.tokenSource === 'env') {
        if (!v.tokenEnv) need('tokenEnv', 'Enter the variable name')
        else if (!ENV_NAME.test(v.tokenEnv)) need('tokenEnv', 'Use letters, digits and underscores, not starting with a digit')
      } else if (!v.token) {
        need('token', 'Paste the API key')
      }
    })
}

/** Comma-separated model ids to a clean list. */
export function parseModels(raw: string | undefined): string[] {
  return (raw ?? '').split(',').map((m) => m.trim()).filter(Boolean)
}
export type AddAccountValues = z.infer<ReturnType<typeof addAccountSchema>>

export const editAccountSchema = z.object({
  cap: z.number({ error: 'Enter a whole number' }).int('Enter a whole number').min(1, 'At least 1').max(16, 'At most 16'),
  priority: z.number({ error: 'Enter a whole number' }).int('Enter a whole number'),
  enabled: z.boolean(),
  pools: z.array(z.string()),
})
export type EditAccountValues = z.infer<typeof editAccountSchema>

export const flagSchema = z
  .object({
    pool: z.string(),
    duration: z.enum(['1', '5', '24', 'custom']),
    until: z.string(),
    reason: z.string().trim().optional(),
  })
  .refine((v) => v.duration !== 'custom' || (v.until !== '' && !Number.isNaN(Date.parse(v.until))), {
    path: ['until'],
    message: 'Pick a date and time',
  })
export type FlagValues = z.infer<typeof flagSchema>

/** The POST /accounts body for validated form values. Endpoint fields are sent only for a claude endpoint account. */
export function buildAddAccountRequest(values: AddAccountValues): AddAccountRequest {
  const base: AddAccountRequest = { harness: values.harness, name: values.name, ...(values.home ? { home: values.home } : {}) }
  const preset = values.endpointPreset ?? 'none'
  if (values.harness !== 'claude' || preset === 'none') return base
  const env = values.tokenSource === 'env'
  return {
    ...base,
    endpoint: {
      preset,
      ...(preset === 'custom'
        ? {
            base_url: values.baseUrl,
            models: parseModels(values.models),
            slots: { opus: values.slotOpus, sonnet: values.slotSonnet, haiku: values.slotHaiku },
          }
        : {}),
      ...(env ? { token_env: values.tokenEnv } : {}),
    },
    ...(env ? {} : { token: values.token?.trim() }),
  }
}

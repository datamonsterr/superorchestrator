import '@/lib/zod-setup'
import { z } from 'zod'
import type { ConfigPatch, ConfigView, DeciderPatch, DeciderView } from '@/api/types'

const whole = (min: number, max?: number) => {
  let n = z.number({ error: 'Enter a whole number' }).int('Enter a whole number').min(min, `At least ${min}`)
  if (max !== undefined) n = n.max(max, `At most ${max}`)
  return n
}

/** The editable subset of `config.toml`, shaped like the config so field names are dotted config keys. */
const baseSchema = z.object({
  limits: z.object({ global_cap: whole(1, 64), max_children: whole(1, 64) }),
  runs: z.object({ stall_ms: whole(1000), ask_timeout: z.string().trim().min(1, 'Enter a duration such as 10m') }),
  notifications: z.object({ desktop: z.boolean(), discord_webhook_env: z.string().trim() }),
  worktrees: z.object({ dir: z.string().trim(), branch_prefix: z.string().trim() }),
  orchestrator: z.object({ reserved_accounts: z.string() }),
  ui: z.object({ bind: z.string().trim().min(1, 'Enter an address'), port: whole(1, 65535), enabled: z.boolean() }),
})
export type SettingsValues = z.infer<typeof baseSchema>

/** The router decider fields (thresholds, key) live in the Decider tab, not here. */
export const settingsSchema = baseSchema

/** Every editable field as a dotted path, read off the schema so the two cannot drift. */
export const FIELD_PATHS: string[] = (() => {
  const walk = (shape: Record<string, z.ZodType>, prefix: string): string[] =>
    Object.entries(shape).flatMap(([k, v]) => (v instanceof z.ZodObject ? walk(v.shape as Record<string, z.ZodType>, `${prefix}${k}.`) : [`${prefix}${k}`]))
  return walk(baseSchema.shape, '')
})()

/** Form values from the server config, with the file's defaults for absent keys. Never reads `ui.token`. */
export function valuesFromConfig(view: ConfigView): SettingsValues {
  const c = view.config
  return {
    limits: { global_cap: c.limits?.global_cap ?? 8, max_children: c.limits?.max_children ?? 5 },
    runs: { stall_ms: c.runs?.stall_ms ?? 300_000, ask_timeout: c.runs?.ask_timeout ?? '10m' },
    notifications: { desktop: c.notifications?.desktop ?? false, discord_webhook_env: c.notifications?.discord_webhook_env ?? '' },
    worktrees: { dir: c.worktrees?.dir ?? '', branch_prefix: c.worktrees?.branch_prefix ?? '' },
    orchestrator: { reserved_accounts: (c.orchestrator?.reserved_accounts ?? []).join(', ') },
    ui: { bind: c.ui?.bind ?? '127.0.0.1', port: c.ui?.port ?? 7878, enabled: c.ui?.enabled ?? false },
  }
}

/** The reserved-accounts field is edited as one comma-separated string. */
const splitNames = (s: string) => s.split(',').map((name) => name.trim()).filter(Boolean)

type Tree = { [k: string]: Tree | string | number | boolean }

/** Only the leaves that changed, as a nested partial config. */
export function diffToPatch(before: SettingsValues, after: SettingsValues): ConfigPatch {
  const walk = (a: Tree, b: Tree, path: string): Record<string, unknown> => {
    const out: Record<string, unknown> = {}
    for (const k of Object.keys(b)) {
      const x = a[k]
      const y = b[k]
      const here = path ? `${path}.${k}` : k
      if (typeof y === 'object') {
        const sub = walk(x as Tree, y, here)
        if (Object.keys(sub).length > 0) out[k] = sub
      } else if (here === 'orchestrator.reserved_accounts' && typeof x === 'string' && typeof y === 'string') {
        const names = splitNames(y)
        if (JSON.stringify(splitNames(x)) !== JSON.stringify(names)) out[k] = names
      } else if (x !== y) {
        out[k] = y
      }
    }
    return out
  }
  return walk(before as unknown as Tree, after as unknown as Tree, '') as ConfigPatch
}

// ---- Decider -------------------------------------------------------------------------------

const LOOPBACK = new Set(['localhost', '127.0.0.1', '[::1]', '::1'])

/** Mirrors the server: https, or http only to a loopback host. */
export function endpointError(raw: string): string | null {
  let u: URL
  try {
    u = new URL(raw)
  } catch {
    return 'Enter a full URL such as https://openrouter.ai/api/v1'
  }
  if (u.protocol === 'https:') return null
  if (u.protocol === 'http:' && LOOPBACK.has(u.hostname)) return null
  return 'Must be https, or http only to a loopback host'
}

const UNIT_SECONDS: Record<string, number> = { ms: 0.001, s: 1, m: 60 }

/** Seconds in a duration like `20s`, `2m` or `1m30s`; null when it is not one. */
export function durationSeconds(raw: string): number | null {
  const s = raw.trim()
  if (!/^(\d+(ms|s|m))+$/.test(s)) return null
  let total = 0
  for (const [, n, unit] of s.matchAll(/(\d+)(ms|s|m)/g)) total += Number(n) * UNIT_SECONDS[unit]
  return total
}

const modelId = (required: boolean) =>
  z
    .string()
    .trim()
    .refine((v) => !/\s/.test(v), 'A model id cannot contain spaces')
    .refine((v) => !required || v !== '', 'Enter a model id')
const fraction = z.number({ error: 'Enter a number between 0 and 1' }).min(0, 'At least 0').max(1, 'At most 1')

export const KEY_MODES = ['env', 'file', 'paste'] as const
export type KeyMode = (typeof KEY_MODES)[number]

export const deciderSchema = z.object({
  endpoint: z.string().trim().min(1, 'Enter an endpoint URL').superRefine((v, ctx) => {
    const m = v ? endpointError(v) : null
    if (m) ctx.addIssue({ code: 'custom', message: m })
  }),
  model: modelId(true),
  fallback_model: modelId(false),
  on_unreachable: z.enum(['error', 'heuristic']),
  timeout: z.string().trim().superRefine((v, ctx) => {
    const s = durationSeconds(v)
    if (s === null) ctx.addIssue({ code: 'custom', message: 'Enter a duration such as 20s' })
    else if (s < 1 || s > 120) ctx.addIssue({ code: 'custom', message: 'Timeout must be from 1s to 120s' })
  }),
  min_confidence: fraction,
  min_margin: fraction,
  key_mode: z.enum(KEY_MODES),
  key_env: z.string().trim(),
  key_file: z.string().trim(),
  key_paste: z.string(),
}).superRefine((v, ctx) => {
  if (v.key_mode === 'env' && v.key_env === '') ctx.addIssue({ code: 'custom', path: ['key_env'], message: 'Enter an environment variable name' })
  if (v.key_mode === 'file' && v.key_file === '') ctx.addIssue({ code: 'custom', path: ['key_file'], message: 'Enter a key file path' })
})
export type DeciderValues = z.infer<typeof deciderSchema>

export const DECIDER_FIELD_PATHS: string[] = Object.keys(deciderSchema.shape)

const DEFAULT_KEY_ENV = 'OPENROUTER_API_KEY'

/** Form values from the server view. The paste field always starts empty: a key is never prefilled. */
export function valuesFromDecider(v: DeciderView): DeciderValues {
  const mode: KeyMode = v.key.source === 'file' ? 'file' : 'env'
  return {
    endpoint: v.endpoint,
    model: v.model,
    fallback_model: v.fallback_model ?? '',
    on_unreachable: v.on_unreachable,
    timeout: v.timeout,
    min_confidence: v.min_confidence,
    min_margin: v.min_margin,
    key_mode: mode,
    key_env: v.key.source === 'env' && v.key.name ? v.key.name : DEFAULT_KEY_ENV,
    key_file: v.key.source === 'file' && v.key.name ? v.key.name : '',
    key_paste: '',
  }
}

/** Only the fields that changed. A cleared fallback model is sent as an empty string, which clears it. */
export function deciderPatch(before: DeciderValues, after: DeciderValues): DeciderPatch {
  const out: DeciderPatch = {}
  if (after.endpoint !== before.endpoint) out.endpoint = after.endpoint
  if (after.model !== before.model) out.model = after.model
  if (after.fallback_model !== before.fallback_model) out.fallback_model = after.fallback_model
  if (after.on_unreachable !== before.on_unreachable) out.on_unreachable = after.on_unreachable
  if (after.timeout !== before.timeout) out.timeout = after.timeout
  if (after.min_confidence !== before.min_confidence) out.min_confidence = after.min_confidence
  if (after.min_margin !== before.min_margin) out.min_margin = after.min_margin
  const moved = after.key_mode !== before.key_mode
  if (after.key_mode === 'paste') {
    if (after.key_paste !== '') out.key = { paste: after.key_paste }
  } else if (after.key_mode === 'env') {
    if (moved || after.key_env !== before.key_env) out.key = { env: after.key_env }
  } else if (moved || after.key_file !== before.key_file) {
    out.key = { file: after.key_file }
  }
  return out
}

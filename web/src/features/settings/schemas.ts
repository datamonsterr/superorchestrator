import { z } from 'zod'
import type { ConfigPatch, ConfigView } from '@/api/types'

const whole = (min: number, max?: number) => {
  let n = z.number({ error: 'Enter a whole number' }).int('Enter a whole number').min(min, `At least ${min}`)
  if (max !== undefined) n = n.max(max, `At most ${max}`)
  return n
}
const fraction = z.number({ error: 'Enter a number between 0 and 1' }).min(0, 'At least 0').max(1, 'At most 1')

/** The editable subset of `config.toml`, shaped like the config so field names are dotted config keys. */
export const settingsSchema = z.object({
  limits: z.object({ global_cap: whole(1, 64), max_children: whole(1, 64) }),
  runs: z.object({ stall_ms: whole(1000), ask_timeout: z.string().trim().min(1, 'Enter a duration such as 10m') }),
  notifications: z.object({ desktop: z.boolean(), discord_webhook_env: z.string().trim() }),
  worktrees: z.object({ dir: z.string().trim(), branch_prefix: z.string().trim() }),
  router: z.object({
    policy: z.object({ min_confidence: fraction, min_margin: fraction }),
    jev: z.object({ key_env: z.string().trim().min(1, 'Enter an environment variable name'), key_file: z.string().trim() }),
  }),
  ui: z.object({ bind: z.string().trim().min(1, 'Enter an address'), port: whole(1, 65535), enabled: z.boolean() }),
})
export type SettingsValues = z.infer<typeof settingsSchema>

/** Form values from the server config, with the file's defaults for absent keys. Never reads `ui.token`. */
export function valuesFromConfig(view: ConfigView): SettingsValues {
  const c = view.config
  return {
    limits: { global_cap: c.limits?.global_cap ?? 8, max_children: c.limits?.max_children ?? 5 },
    runs: { stall_ms: c.runs?.stall_ms ?? 300_000, ask_timeout: c.runs?.ask_timeout ?? '10m' },
    notifications: { desktop: c.notifications?.desktop ?? false, discord_webhook_env: c.notifications?.discord_webhook_env ?? '' },
    worktrees: { dir: c.worktrees?.dir ?? '', branch_prefix: c.worktrees?.branch_prefix ?? '' },
    router: {
      policy: { min_confidence: c.router?.policy?.min_confidence ?? 0.5, min_margin: c.router?.policy?.min_margin ?? 0.1 },
      jev: { key_env: c.router?.jev?.key_env ?? 'OPENROUTER_API_KEY', key_file: c.router?.jev?.key_file ?? '' },
    },
    ui: { bind: c.ui?.bind ?? '127.0.0.1', port: c.ui?.port ?? 7878, enabled: c.ui?.enabled ?? false },
  }
}

type Tree = { [k: string]: Tree | string | number | boolean }

/**
 * Only the leaves that changed, as a nested partial config. An optional string that was cleared
 * (`key_file`) is not sent: the API has no way to unset a key, so it keeps its value.
 */
export function diffToPatch(before: SettingsValues, after: SettingsValues): ConfigPatch {
  const walk = (a: Tree, b: Tree): Tree => {
    const out: Tree = {}
    for (const k of Object.keys(b)) {
      const x = a[k]
      const y = b[k]
      if (typeof y === 'object') {
        const sub = walk(x as Tree, y)
        if (Object.keys(sub).length > 0) out[k] = sub
      } else if (x !== y && !(k === 'key_file' && y === '')) {
        out[k] = y
      }
    }
    return out
  }
  return walk(before as unknown as Tree, after as unknown as Tree) as ConfigPatch
}

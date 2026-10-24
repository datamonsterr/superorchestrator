import '@/lib/zod-setup'
import { z } from 'zod'
import type { Harness, Level, RouteParams } from '@/api/types'

export const HARNESSES = ['codex', 'agy', 'opencode', 'claude'] as const
export const TIERS = ['high', 'mid', 'fast'] as const

/** `harness/model[/effort]`: the ref a Choice is written as in `config.toml` and on the wire. */
export function buildRef(harness: string, model: string, effort?: string): string {
  const m = model.trim()
  const e = effort?.trim()
  return e ? `${harness}/${m}/${e}` : `${harness}/${m}`
}

/**
 * The inverse of `buildRef`. OpenCode models are `provider/model`, so a third segment is the
 * effort; for the other harnesses a model has no slash, so a second segment is the effort.
 * The server stays the judge of what a ref means: this only fills the model dialog when editing.
 */
export function splitRef(ref: string): { harness: Harness; model: string; effort: string } {
  const [harness, ...rest] = ref.split('/')
  const modelSegments = harness === 'opencode' ? 2 : 1
  return {
    harness: harness as Harness,
    model: rest.slice(0, modelSegments).join('/'),
    effort: rest.slice(modelSegments).join('/'),
  }
}

/** The fields of the "Add model" dialog; `existing` are the refs the level already lists. */
export function modelRefSchema(existing: readonly string[]) {
  return z
    .object({
      harness: z.enum(HARNESSES),
      model: z.string().trim().min(1, 'Enter a model'),
      effort: z.string().trim(),
    })
    .superRefine((v, ctx) => {
      if (v.model && v.harness === 'opencode' && !v.model.includes('/')) {
        ctx.addIssue({ code: 'custom', path: ['model'], message: 'OpenCode models are provider/model, for example opencode/muse-spark-1.3' })
      } else if (v.model && existing.includes(buildRef(v.harness, v.model, v.effort))) {
        ctx.addIssue({ code: 'custom', path: ['model'], message: 'This level already lists this model' })
      }
    })
}
export type ModelRefValues = z.infer<ReturnType<typeof modelRefSchema>>

/** Name and range of a Level; `others` are the other levels, so duplicates and overlaps show before the server says so. */
export function levelSchema(others: readonly Pick<Level, 'name' | 'min' | 'max'>[]) {
  const bound = (label: string) => z.number({ error: `Enter ${label} from 0 to 100` }).int('Use a whole number').min(0, 'Use 0 to 100').max(100, 'Use 0 to 100')
  return z
    .object({
      name: z.string().trim().min(1, 'Enter a name'),
      min: bound('a start'),
      max: bound('an end'),
      tier: z.string(),
    })
    .superRefine((v, ctx) => {
      if (others.some((o) => o.name.trim() === v.name)) ctx.addIssue({ code: 'custom', path: ['name'], message: 'A level with this name already exists' })
      if (v.min > v.max) ctx.addIssue({ code: 'custom', path: ['max'], message: 'The end must not be below the start' })
      else {
        const clash = others.find((o) => o.min <= v.max && v.min <= o.max)
        if (clash) ctx.addIssue({ code: 'custom', path: ['max'], message: `Overlaps ${clash.name} (${clash.min}–${clash.max})` })
      }
    })
}
export type LevelValues = z.infer<ReturnType<typeof levelSchema>>

/** The select value for "no tier". */
export const NO_TIER = 'none'

export const previewSchema = z.object({
  prompt: z.string().trim().min(1, 'Enter a prompt to route'),
  harness: z.string(),
  account: z.string(),
  model: z.string().trim(),
  effort: z.string().trim(),
})
export type PreviewValues = z.infer<typeof previewSchema>

export const ANY = 'any'

export function paramsFrom(v: PreviewValues): RouteParams {
  return {
    prompt: v.prompt,
    ...(v.harness !== ANY ? { harness: v.harness } : {}),
    ...(v.account !== ANY ? { account: v.account } : {}),
    ...(v.model ? { model: v.model } : {}),
    ...(v.effort ? { effort: v.effort } : {}),
  }
}

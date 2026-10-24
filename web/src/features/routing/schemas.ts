import '@/lib/zod-setup'
import { z } from 'zod'
import type { Choice, Level, LevelChoice, LevelView, RouteParams } from '@/api/types'

export const HARNESSES = ['codex', 'agy', 'opencode', 'claude'] as const

export function choiceSchema(otherIds: readonly string[]) {
  return z.object({
    id: z
      .string()
      .min(1, 'Enter an id')
      .regex(/^[a-z0-9-]+$/, 'Use lowercase letters, digits and dashes')
      .refine((v) => !otherIds.includes(v), 'A choice with this id already exists'),
    harness: z.enum(HARNESSES),
    model: z.string().trim().min(1, 'Enter a model'),
    effort: z.string().trim(),
    roles: z.string().refine((s) => parseRoles(s).length > 0, 'Enter at least one role'),
    description: z.string().trim(),
    enabled: z.boolean(),
  })
}
export type ChoiceValues = z.infer<ReturnType<typeof choiceSchema>>

/** "code, review" -> ["code", "review"]; blanks and duplicates dropped. */
export function parseRoles(csv: string): string[] {
  return [...new Set(csv.split(',').map((r) => r.trim()).filter(Boolean))]
}

export function toChoice(v: ChoiceValues): Choice {
  return {
    id: v.id,
    harness: v.harness,
    model: v.model.trim(),
    ...(v.effort ? { effort: v.effort } : {}),
    roles: parseRoles(v.roles),
    description: v.description,
    enabled: v.enabled,
  }
}

export function fromChoice(c: Choice): ChoiceValues {
  return { id: c.id, harness: c.harness, model: c.model, effort: c.effort ?? '', roles: c.roles.join(', '), description: c.description, enabled: c.enabled }
}

export const previewSchema = z.object({
  prompt: z.string().trim().min(1, 'Enter a prompt to route'),
  role: z.string().trim(),
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
    ...(v.role ? { role: v.role } : {}),
    ...(v.harness !== ANY ? { harness: v.harness } : {}),
    ...(v.account !== ANY ? { account: v.account } : {}),
    ...(v.model ? { model: v.model } : {}),
    ...(v.effort ? { effort: v.effort } : {}),
  }
}


export const TIERS = ['high', 'mid', 'fast'] as const
export const NO_TIER = 'none'

const range = (label: string) =>
  z
    .string()
    .trim()
    .min(1, `Enter ${label}`)
    .refine((s) => /^\d+$/.test(s) && Number(s) <= 100, `${label[0].toUpperCase()}${label.slice(1)} must be a whole number from 0 to 100`)

/** `otherNames`: the other levels' names. `hasChoices`: the level lists models, so a tier is optional. */
export function levelSchema(otherNames: readonly string[], hasChoices: boolean) {
  return z
    .object({
      name: z
        .string()
        .trim()
        .min(1, 'Enter a name')
        .refine((v) => !otherNames.includes(v), 'A level with this name already exists'),
      min: range('a minimum'),
      max: range('a maximum'),
      tier: z.enum([NO_TIER, ...TIERS]),
    })
    .superRefine((v, ctx) => {
      if (/^\d+$/.test(v.min) && /^\d+$/.test(v.max) && Number(v.min) > Number(v.max)) ctx.addIssue({ code: 'custom', path: ['max'], message: 'Maximum must not be below the minimum' })
      if (!hasChoices && v.tier === NO_TIER) ctx.addIssue({ code: 'custom', path: ['tier'], message: 'Pick a tier, or add models to the level' })
    })
}
export type LevelValues = z.infer<ReturnType<typeof levelSchema>>

export function fromLevel(l: LevelView | null): LevelValues {
  return l ? { name: l.name, min: String(l.min), max: String(l.max), tier: l.tier ?? NO_TIER } : { name: '', min: '0', max: '100', tier: NO_TIER }
}

/** Form values -> `Level`, keeping the models the level already lists. */
export function toLevel(v: LevelValues, choices: LevelChoice[]): Level {
  return { name: v.name.trim(), min: Number(v.min), max: Number(v.max), ...(v.tier === NO_TIER ? {} : { tier: v.tier }), choices }
}

/** What the server resolved -> what `PUT /levels` takes back, in display order. */
export function levelOf(l: LevelView): Level {
  return { name: l.name, min: l.min, max: l.max, ...(l.tier ? { tier: l.tier } : {}), choices: l.choices.map((c) => ({ id: c.id, ...(c.priority != null ? { priority: c.priority } : {}) })) }
}

export const addModelSchema = z.object({
  choice: z.string().min(1, 'Pick a choice'),
  priority: z.string().trim().refine((s) => s === '' || /^\d+$/.test(s), 'Priority must be a whole number'),
})
export type AddModelValues = z.infer<typeof addModelSchema>

/** `id — harness / model / effort`, for a select option. */
export const choiceLabel = (c: Choice) => `${c.id} — ${c.harness} / ${c.model}${c.effort ? ` / ${c.effort}` : ''}`

import { z } from 'zod'
import type { Choice, RouteParams } from '@/api/types'

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


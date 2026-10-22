import { z } from 'zod'

export const HARNESSES = ['codex', 'agy', 'opencode', 'claude'] as const
export const AGY_POOLS = ['gemini', 'claude'] as const

/** Account names double as directory and wrapper names (design 5). */
export function addAccountSchema(existing: readonly string[]) {
  return z.object({
    harness: z.enum(HARNESSES),
    name: z
      .string()
      .min(1, 'Enter a name')
      .regex(/^[a-z0-9-]+$/, 'Use lowercase letters, digits and dashes')
      .refine((n) => !existing.includes(n), 'An account with this name already exists'),
    home: z.string().trim().optional(),
  })
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

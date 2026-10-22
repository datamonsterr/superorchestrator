import { useEffect } from 'react'
import { useForm, useWatch, type FieldPath } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { TabsContent } from '@/components/ui/tabs'
import { usePatchConfig } from '@/api/queries'
import { applyApiFieldErrors } from '@/lib/form-errors'
import { errorMessage } from '@/lib/errors'
import type { ConfigView } from '@/api/types'
import { KeySource } from './KeySource'
import { diffToPatch, settingsSchema, valuesFromConfig, type SettingsValues } from './schemas'

type Name = FieldPath<SettingsValues>

export function ConfigForm({ view }: { view: ConfigView }) {
  const patch = usePatchConfig()
  const initial = valuesFromConfig(view)
  const form = useForm<SettingsValues>({ resolver: zodResolver(settingsSchema), defaultValues: initial })
  const { control, register, handleSubmit, reset, setError, setValue, formState } = form
  const switches = { 'notifications.desktop': useWatch({ control, name: 'notifications.desktop' }), 'ui.enabled': useWatch({ control, name: 'ui.enabled' }) }
  const { errors, isDirty } = formState

  // A refetch after a save (or on focus) brings the file's values; keep edits in progress.
  useEffect(() => {
    if (!form.formState.isDirty) reset(valuesFromConfig(view))
  }, [view, form.formState.isDirty, reset])

  async function onSubmit(values: SettingsValues) {
    const body = diffToPatch(valuesFromConfig(view), values)
    if (Object.keys(body).length === 0) return
    try {
      const next = await patch.mutateAsync(body)
      reset(valuesFromConfig(next))
      toast.success(next.restart_required ? 'Saved. Restart the supervisor to apply.' : 'Settings saved')
    } catch (e) {
      const fields = Object.keys(settingsFlat)
      if (!applyApiFieldErrors(e, setError, fields)) setError('root', { type: 'server', message: errorMessage(e) })
    }
  }

  const text = (name: Name, label: string, hint?: string, props: { type?: string; number?: boolean; mono?: boolean } = {}) => {
    const err = name.split('.').reduce<unknown>((o, k) => (o as Record<string, unknown> | undefined)?.[k], errors) as { message?: string } | undefined
    return (
      <Field data-invalid={!!err}>
        <FieldLabel htmlFor={`cfg-${name}`}>{label}</FieldLabel>
        <Input
          id={`cfg-${name}`}
          type={props.number ? 'number' : 'text'}
          inputMode={props.number ? 'decimal' : undefined}
          step={props.number ? 'any' : undefined}
          aria-invalid={!!err}
          autoComplete="off"
          className={props.mono ? 'font-mono' : undefined}
          {...register(name, props.number ? { valueAsNumber: true } : undefined)}
        />
        {hint && <FieldDescription>{hint}</FieldDescription>}
        <FieldError errors={[err]} />
      </Field>
    )
  }
  const toggle = (name: 'notifications.desktop' | 'ui.enabled', label: string, hint: string) => (
    <Field orientation="horizontal">
      <Switch id={`cfg-${name}`} checked={switches[name]} onCheckedChange={(v) => setValue(name, v, { shouldDirty: true })} />
      <div className="flex flex-col gap-1">
        <FieldLabel htmlFor={`cfg-${name}`}>{label}</FieldLabel>
        <FieldDescription>{hint}</FieldDescription>
      </div>
    </Field>
  )

  return (
    <form onSubmit={handleSubmit(onSubmit)} noValidate className="space-y-4">
      <TabsContent value="general">
        <FieldGroup className="max-w-xl">
          {text('limits.global_cap', 'Global run cap', 'Runs allowed at the same time across all accounts.', { number: true })}
          {text('limits.max_children', 'Max children per run', 'How many runs one run may spawn.', { number: true })}
          {text('runs.stall_ms', 'Stall timeout (ms)', 'A run with no output for this long is marked stalled.', { number: true })}
          {text('runs.ask_timeout', 'Question timeout', 'How long a run waits for an answer, for example 10m.')}
        </FieldGroup>
      </TabsContent>
      <TabsContent value="notifications">
        <FieldGroup className="max-w-xl">
          {toggle('notifications.desktop', 'Desktop notifications', 'Notify when a run needs attention.')}
          {text('notifications.discord_webhook_env', 'Discord webhook variable', 'The name of the environment variable that holds the webhook URL. The URL itself is never shown.', { mono: true })}
        </FieldGroup>
      </TabsContent>
      <TabsContent value="worktrees">
        <FieldGroup className="max-w-xl">
          {text('worktrees.dir', 'Worktree folder', 'Where run worktrees are created. Empty uses the default.', { mono: true })}
          {text('worktrees.branch_prefix', 'Branch prefix', 'Prefix for the branches runs work on.', { mono: true })}
        </FieldGroup>
      </TabsContent>
      <TabsContent value="routing">
        <FieldGroup className="max-w-xl">
          {text('router.policy.min_confidence', 'Minimum confidence', 'Below this the router falls back to the heuristic (0 to 1).', { number: true })}
          {text('router.policy.min_margin', 'Minimum margin', 'The lead the top choice needs over the runner-up (0 to 1).', { number: true })}
          <KeySource label="OpenRouter key" source={view.openrouter_key} />
          {text('router.jev.key_env', 'Key environment variable', 'The variable the key is read from. Pasting a key here is not supported.', { mono: true })}
          {text('router.jev.key_file', 'Key file', 'Optional dotenv file holding the key. Clearing it here does not remove the saved path.', { mono: true })}
        </FieldGroup>
      </TabsContent>
      <TabsContent value="ui">
        <FieldGroup className="max-w-xl">
          {text('ui.bind', 'Bind address', 'Changing the address or port takes effect after a restart.', { mono: true })}
          {text('ui.port', 'Port', undefined, { number: true })}
          {toggle('ui.enabled', 'Start with the supervisor', 'Start the web UI whenever the supervisor starts.')}
          <KeySource label="UI token" source={view.ui_token} />
        </FieldGroup>
      </TabsContent>
      {Object.keys(errors).some((k) => k !== 'root') && (
        <p role="alert" className="text-sm text-destructive">
          Some settings are invalid. Check every tab.
        </p>
      )}
      {errors.root && (
        <p role="alert" className="text-sm text-destructive">
          {errors.root.message}
        </p>
      )}
      <div className="flex items-center gap-2">
        <Button type="submit" disabled={!isDirty || patch.isPending}>
          {patch.isPending ? 'Saving...' : 'Save'}
        </Button>
        <Button type="button" variant="ghost" disabled={!isDirty || patch.isPending} onClick={() => reset(valuesFromConfig(view))}>
          Discard
        </Button>
      </div>
    </form>
  )
}

/** Dotted names that a 422 `data` object may point at. */
const settingsFlat: Record<string, true> = Object.fromEntries(
  [
    'limits.global_cap', 'limits.max_children', 'runs.stall_ms', 'runs.ask_timeout', 'notifications.desktop', 'notifications.discord_webhook_env',
    'worktrees.dir', 'worktrees.branch_prefix', 'router.policy.min_confidence', 'router.policy.min_margin', 'router.jev.key_env', 'router.jev.key_file',
    'ui.bind', 'ui.port', 'ui.enabled',
  ].map((k) => [k, true] as const),
)

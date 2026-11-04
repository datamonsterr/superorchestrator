import { useEffect, useId, useState, type ReactNode } from 'react'
import { useForm, useWatch, type FieldPath } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { Loader2 } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel, FieldLegend, FieldSet } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { TabsContent } from '@/components/ui/tabs'
import { usePatchDecider, useTestDecider } from '@/api/queries'
import { errorMessage } from '@/lib/errors'
import type { DeciderTestResult, DeciderView, EnvOverride } from '@/api/types'
import { DECIDER_FIELD_PATHS, deciderPatch, deciderSchema, valuesFromDecider, type DeciderValues, type KeyMode } from './schemas'

type Name = FieldPath<DeciderValues>

const SELECT_CLASS =
  'h-8 w-full rounded-lg border border-input bg-transparent px-2.5 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 aria-invalid:border-destructive md:text-sm dark:bg-input/30'

const KEY_MODE_LABELS: Record<KeyMode, string> = { env: 'Environment variable', file: 'Key file', paste: 'Paste key' }

/** Where the key comes from now. Only the origin, its name and the masked tail: never the value. */
function keySummary(v: DeciderView): string {
  const { source, name, masked } = v.key
  if (source === 'unset') return `No key found. Set ${name ?? 'OPENROUTER_API_KEY'} in the supervisor's environment, or choose a key file or paste one below.`
  const from = source === 'env' ? `From environment variable ${name ?? ''}` : `From file ${name ?? ''}`
  return masked ? `${from}: ${masked}` : from
}

function Override({ o }: { o: EnvOverride }) {
  if (o.field === 'key_file') {
    return (
      <p role="note" className="text-sm text-amber-700 dark:text-amber-400" data-testid="env-override-key_file">
        <code className="font-mono">{o.name}</code> is set and wins over the key file until it is unset.
      </p>
    )
  }
  return (
    <p role="note" className="text-sm text-amber-700 dark:text-amber-400" data-testid={`env-override-${o.field}`}>
      <code className="font-mono">{o.name}</code> is set{o.value ? ` (${o.value})` : ''} and wins over this setting until it is unset.
    </p>
  )
}

/** The request itself failed (no answer from the supervisor), so there is no latency to show. */
type TestState = DeciderTestResult | { transportError: string }

function TestResult({ r }: { r: TestState }) {
  if ('transportError' in r) {
    return (
      <p role="alert" className="text-sm text-destructive">
        Test failed: {r.transportError}
      </p>
    )
  }
  if (!r.ok) {
    return (
      <p role="alert" className="text-sm text-destructive">
        Test failed: {r.error ?? 'the decider did not answer'} ({r.latency_ms} ms)
      </p>
    )
  }
  const rows: [string, string][] = [
    ['Complexity', r.complexity == null ? '-' : String(r.complexity)],
    ['Level', r.level ?? '-'],
    ['Confidence', r.confidence == null ? '-' : String(r.confidence)],
    ['Model', r.model ?? '-'],
    ['Latency', `${r.latency_ms} ms`],
  ]
  return (
    <dl role="status" aria-label="Test result" className="grid max-w-xl grid-cols-[auto_1fr] gap-x-4 gap-y-1 rounded-md border p-3 text-sm">
      {rows.map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="text-muted-foreground">{k}</dt>
          <dd className="font-mono">{v}</dd>
        </div>
      ))}
    </dl>
  )
}

/** The decider settings. `hidden` keeps the draft mounted while another tab is shown. */
export function DeciderForm({ view, hidden = false }: { view: DeciderView; hidden?: boolean }) {
  const patch = usePatchDecider()
  const test = useTestDecider()
  const [prompt, setPrompt] = useState('')
  const [result, setResult] = useState<TestState | null>(null)
  const uid = useId()
  const form = useForm<DeciderValues>({ resolver: zodResolver(deciderSchema), defaultValues: valuesFromDecider(view) })
  const { control, register, handleSubmit, reset, setError, setValue, formState } = form
  const { errors, isDirty } = formState
  const mode = useWatch({ control, name: 'key_mode' })
  const overrideFor = (field: string) => view.env_overrides.filter((o) => o.field === field)

  // A pasted key must not outlive the paste choice, so leaving it drops the text.
  useEffect(() => {
    if (mode !== 'paste') setValue('key_paste', '')
  }, [mode, setValue])

  useEffect(() => {
    if (hidden) setValue('key_paste', '')
    return () => setValue('key_paste', '')
  }, [hidden, setValue])

  useEffect(() => {
    if (!form.formState.isDirty) reset(valuesFromDecider(view))
  }, [view, form.formState.isDirty, reset])

  async function onSubmit(values: DeciderValues) {
    const body = deciderPatch(valuesFromDecider(view), values)
    if (Object.keys(body).length === 0) {
      toast('Nothing to save')
      reset(valuesFromDecider(view))
      return
    }
    try {
      const next = await patch.save(body)
      reset(valuesFromDecider(next)) // also empties the paste field
      toast.success(next.restart_required ? 'Saved. Restart the supervisor to apply.' : 'Decider settings saved')
    } catch (e) {
      // The server names the field first ("endpoint: must be https ...").
      const message = errorMessage(e)
      const field = message.split(':', 1)[0]
      if (message.includes(':') && DECIDER_FIELD_PATHS.includes(field)) setError(field as Name, { type: 'server', message: message.slice(field.length + 1).trim() })
      else setError('root', { type: 'server', message })
    } finally {
      setValue('key_paste', '')
    }
  }

  async function runTest() {
    setResult(null)
    try {
      setResult(await test.mutateAsync(prompt.trim() ? { prompt: prompt.trim() } : {}))
    } catch (e) {
      setResult({ transportError: errorMessage(e) })
    }
  }

  const errOf = (name: Name) => errors[name] as { message?: string } | undefined
  const field = (name: Name, label: string, hint: ReactNode, opts: { number?: boolean; mono?: boolean; list?: string; override?: string; type?: string } = {}) => {
    const err = errOf(name)
    const id = `decider-${name}`
    return (
      <Field data-invalid={!!err}>
        <FieldLabel htmlFor={id}>{label}</FieldLabel>
        <Input
          id={id}
          type={opts.type ?? (opts.number ? 'number' : 'text')}
          inputMode={opts.number ? 'decimal' : undefined}
          step={opts.number ? 'any' : undefined}
          list={opts.list}
          aria-invalid={!!err}
          aria-describedby={`${id}-hint`}
          autoComplete="off"
          className={opts.mono ? 'font-mono' : undefined}
          {...(opts.type === 'password'
            ? { autoComplete: 'new-password', 'data-1p-ignore': true, 'data-lpignore': 'true', spellCheck: false }
            : opts.mono
              ? { spellCheck: false, autoCapitalize: 'off' }
              : {})}
          {...register(name, opts.number ? { valueAsNumber: true } : undefined)}
        />
        <FieldDescription id={`${id}-hint`}>{hint}</FieldDescription>
        {opts.override && overrideFor(opts.override).map((o) => <Override key={o.name} o={o} />)}
        <FieldError errors={[err]} />
      </Field>
    )
  }

  const eff = view.effective
  return (
    <form onSubmit={handleSubmit(onSubmit)} noValidate hidden={hidden} className="space-y-4" aria-label="Decider settings">
      <TabsContent value="decider">
        <FieldGroup className="max-w-xl">
          <p className="text-sm text-muted-foreground">
            The decider scores each task&apos;s complexity; the router maps that score to a level.
          </p>
          <div data-testid="decider-effective" className="rounded-md border p-3 text-sm">
            <p className="font-medium">Running supervisor models</p>
            <p>
              Model <span className="font-mono">{eff.primary}</span> ({eff.primary_source})
            </p>
            <p>
              Fallback {eff.fallback ? <span className="font-mono">{eff.fallback}</span> : 'none'} ({eff.fallback_source})
            </p>
          </div>

          {field('endpoint', 'Endpoint', 'An https URL, or http to a loopback host such as localhost.', { mono: true })}
          {field('model', 'Model', 'The model that scores complexity. Any OpenRouter model id.', { mono: true, list: `${uid}-models`, override: 'model' })}
          {field('fallback_model', 'Fallback model', 'Tried when the model fails. Empty for none.', { mono: true, list: `${uid}-fallbacks`, override: 'fallback_model' })}
          <datalist id={`${uid}-models`}>
            <option value={view.suggestions.model} />
          </datalist>
          <datalist id={`${uid}-fallbacks`}>{view.suggestions.fallback_model && <option value={view.suggestions.fallback_model} />}</datalist>

          <Field data-invalid={!!errOf('on_unreachable')}>
            <FieldLabel htmlFor="decider-on_unreachable">When the decider is unreachable</FieldLabel>
            {/* Native select: two fixed options, and it keeps the form testable without a popper. */}
            <select id="decider-on_unreachable" className={SELECT_CLASS} aria-describedby="decider-on_unreachable-hint" {...register('on_unreachable')}>
              <option value="error">error</option>
              <option value="heuristic">heuristic</option>
            </select>
            <FieldDescription id="decider-on_unreachable-hint">
              error: fail the route. heuristic: estimate the complexity locally and carry on.
            </FieldDescription>
            <FieldError errors={[errOf('on_unreachable')]} />
          </Field>

          {field('timeout', 'Timeout', 'How long to wait for the decider, from 1s to 120s, for example 20s.', { mono: true })}
          {field('min_confidence', 'Minimum confidence', "Below this the decider's score is ignored and the heuristic estimates the complexity (0 to 1).", { number: true })}
          {field('min_margin', 'Minimum margin', 'Deprecated: routing no longer uses it. Kept so older configs still load (0 to 1).', { number: true })}

          <FieldSet>
            <FieldLegend variant="label">OpenRouter key</FieldLegend>
            <p id="decider-key-current" data-testid="decider-key-current" className="text-sm text-muted-foreground">
              {keySummary(view)}
            </p>
            {overrideFor('key_file').map((o) => <Override key={o.name} o={o} />)}
            <div role="radiogroup" aria-label="Key source" aria-describedby="decider-key-current" className="flex flex-wrap gap-4 text-sm">
              {(Object.keys(KEY_MODE_LABELS) as KeyMode[]).map((m) => (
                <label key={m} className="flex items-center gap-2">
                  <input type="radio" value={m} {...register('key_mode')} />
                  {KEY_MODE_LABELS[m]}
                </label>
              ))}
            </div>
            {mode === 'env' && field('key_env', 'Environment variable name', 'The variable the key is read from.', { mono: true })}
            {mode === 'file' && field('key_file', 'Key file path', 'A dotenv file holding the key.', { mono: true })}
            {mode === 'paste' &&
              field('key_paste', 'Key', 'Stored in the supervisor, never shown again. Leave empty to keep the current key.', { type: 'password', mono: true })}
          </FieldSet>
        </FieldGroup>

        <FieldGroup className="mt-6 max-w-xl">
          <Field>
            <FieldLabel htmlFor="decider-test-prompt">Test prompt (optional)</FieldLabel>
            <Textarea id="decider-test-prompt" value={prompt} onChange={(e) => setPrompt(e.target.value)} aria-describedby="decider-test-hint" />
            <FieldDescription id="decider-test-hint">Runs the saved settings once. Save first to test changes.</FieldDescription>
          </Field>
          <div>
            <Button type="button" variant="outline" disabled={test.isPending} onClick={() => void runTest()}>
              {test.isPending && <Loader2 className="animate-spin" aria-hidden />}
              {test.isPending ? 'Testing...' : 'Test decider'}
            </Button>
          </div>
          {result && <TestResult r={result} />}
        </FieldGroup>
      </TabsContent>

      {Object.keys(errors).some((k) => k !== 'root') && (
        <p role="alert" className="text-sm text-destructive">
          Some settings are invalid.
        </p>
      )}
      {errors.root && (
        <p role="alert" className="text-sm text-destructive">
          {errors.root.message}
        </p>
      )}
      <div data-slot="form-actions" className="flex items-center gap-2 max-md:[&_button]:min-h-10">
        <Button type="submit" disabled={!isDirty || patch.isPending}>
          {patch.isPending ? 'Saving...' : 'Save'}
        </Button>
        <Button type="button" variant="ghost" disabled={!isDirty || patch.isPending} onClick={() => reset(valuesFromDecider(view))}>
          Discard
        </Button>
      </div>
    </form>
  )
}

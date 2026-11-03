import { useMemo, useState } from 'react'
import { Controller, useForm, useWatch } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { ChevronDown } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel, FieldLegend, FieldSet } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useConfig, useCreateAccount } from '@/api/queries'
import { applyApiFieldErrors } from '@/lib/form-errors'
import type { AccountView, AddAccountRequest } from '@/api/types'
import { addAccountSchema, ENDPOINT_PRESETS, HARNESSES, parseModels, type AddAccountValues, type EndpointPreset } from './schemas'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  accounts: AccountView[]
  /** Called with the new account name so the page can open the sign-in step. */
  onCreated: (name: string) => void
}

const PRESET_LABEL: Record<EndpointPreset, string> = {
  none: 'None (Anthropic subscription)',
  zai: 'Z.ai GLM',
  kimi: 'Kimi Code',
  minimax: 'MiniMax',
  custom: 'Custom',
}
const PRESET_URL: Record<Exclude<EndpointPreset, 'none' | 'custom'>, string> = {
  zai: 'https://api.z.ai/api/anthropic',
  kimi: 'https://api.kimi.ai/coding/',
  minimax: 'https://api.minimax.io/anthropic',
}
const DEFAULTS: AddAccountValues = {
  harness: 'codex',
  name: '',
  home: '',
  endpointPreset: 'none',
  baseUrl: '',
  models: '',
  slotOpus: '',
  slotSonnet: '',
  slotHaiku: '',
  tokenSource: 'paste',
  token: '',
  tokenEnv: '',
}

// API error keys (dotted for the nested endpoint object) to form fields.
const API_FIELD: Record<string, keyof AddAccountValues> = {
  harness: 'harness',
  name: 'name',
  home: 'home',
  token: 'token',
  endpoint: 'endpointPreset',
  'endpoint.preset': 'endpointPreset',
  'endpoint.base_url': 'baseUrl',
  'endpoint.models': 'models',
  'endpoint.slots': 'slotOpus',
  'endpoint.token_env': 'tokenEnv',
}
const ENDPOINT_ERROR_FIELDS = Object.keys(API_FIELD)

const LABEL: Record<(typeof HARNESSES)[number], string> = { codex: 'Codex', agy: 'Antigravity (agy)', opencode: 'OpenCode', claude: 'Claude' }

export function AddAccountDialog({ open, onOpenChange, accounts, onCreated }: Props) {
  const config = useConfig()
  const create = useCreateAccount()
  const [advanced, setAdvanced] = useState(false)
  const schema = useMemo(() => addAccountSchema(accounts.map((a) => a.name)), [accounts])
  const form = useForm<AddAccountValues>({ resolver: zodResolver(schema), defaultValues: DEFAULTS })
  const { control, register, handleSubmit, setError, formState } = form
  // macOS keeps one agy account per machine (SPEC 6.4).
  const agyBlocked = config.data?.platform === 'macos' && accounts.some((a) => a.harness === 'agy')
  const harness = useWatch({ control, name: 'harness' })
  const blocked = harness === 'agy' && agyBlocked
  const preset = useWatch({ control, name: 'endpointPreset' }) ?? 'none'
  const tokenSource = useWatch({ control, name: 'tokenSource' }) ?? 'paste'
  const isEndpoint = harness === 'claude' && preset !== 'none'

  async function onSubmit(values: AddAccountValues) {
    try {
      const base = { harness: values.harness, name: values.name, ...(values.home ? { home: values.home } : {}) }
      let body: AddAccountRequest = base
      if (values.harness === 'claude' && values.endpointPreset && values.endpointPreset !== 'none') {
        const custom = values.endpointPreset === 'custom'
        const env = values.tokenSource === 'env'
        body = {
          ...base,
          endpoint: {
            preset: values.endpointPreset,
            ...(custom
              ? {
                  base_url: values.baseUrl,
                  models: parseModels(values.models),
                  slots: { opus: values.slotOpus, sonnet: values.slotSonnet, haiku: values.slotHaiku },
                }
              : {}),
            ...(env ? { token_env: values.tokenEnv } : {}),
          },
          ...(env ? {} : { token: values.token }),
        }
      }
      const account = await create.mutateAsync(body)
      changeOpen(false)
      onCreated(account.name)
    } catch (e) {
      applyApiFieldErrors(e, (name, error) => setError(API_FIELD[name] ?? name, error), ENDPOINT_ERROR_FIELDS)
      // The token never stays in form state after a submit, even a failed one.
      form.setValue('token', '')
    }
  }

  function changeOpen(o: boolean) {
    if (!o) {
      form.reset()
      setAdvanced(false)
    }
    onOpenChange(o)
  }

  return (
    <Dialog
      open={open}
      onOpenChange={changeOpen}
    >
      <DialogContent>
        <form onSubmit={handleSubmit(onSubmit)} noValidate className="flex flex-col gap-4">
          <DialogHeader>
            <DialogTitle>Add account</DialogTitle>
            <DialogDescription>Creates the credential home and wrapper, then starts sign-in.</DialogDescription>
          </DialogHeader>
          <FieldGroup>
            <Field data-invalid={!!formState.errors.harness}>
              <FieldLabel htmlFor="add-harness">Harness</FieldLabel>
              <Controller
                control={control}
                name="harness"
                render={({ field }) => (
                  <Select value={field.value} onValueChange={field.onChange}>
                    <SelectTrigger id="add-harness" className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {HARNESSES.map((h) => (
                        <SelectItem key={h} value={h} disabled={h === 'agy' && agyBlocked}>
                          {LABEL[h]}
                          {h === 'agy' && agyBlocked ? ' (one per macOS machine)' : ''}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              />
              <FieldError errors={[formState.errors.harness]} />
            </Field>
            <Field data-invalid={!!formState.errors.name}>
              <FieldLabel htmlFor="add-name">Name</FieldLabel>
              <Input id="add-name" autoComplete="off" placeholder="codex-acc2" aria-invalid={!!formState.errors.name} {...register('name')} />
              <FieldDescription>Lowercase letters, digits and dashes.</FieldDescription>
              <FieldError errors={[formState.errors.name]} />
            </Field>
            {harness === 'claude' && (
              <Field data-invalid={!!formState.errors.endpointPreset}>
                <FieldLabel htmlFor="add-endpoint">Endpoint</FieldLabel>
                <Controller
                  control={control}
                  name="endpointPreset"
                  render={({ field }) => (
                    <Select value={field.value ?? 'none'} onValueChange={field.onChange}>
                      <SelectTrigger id="add-endpoint" className="w-full">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {ENDPOINT_PRESETS.map((p) => (
                          <SelectItem key={p} value={p}>
                            {PRESET_LABEL[p]}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  )}
                />
                <FieldError errors={[formState.errors.endpointPreset]} />
              </Field>
            )}
            {isEndpoint && (
              <>
                {preset === 'custom' ? (
                  <>
                    <Field data-invalid={!!formState.errors.baseUrl}>
                      <FieldLabel htmlFor="add-base-url">Base URL</FieldLabel>
                      <Input id="add-base-url" autoComplete="off" placeholder="https://api.example.com/anthropic" aria-invalid={!!formState.errors.baseUrl} {...register('baseUrl')} />
                      <FieldError errors={[formState.errors.baseUrl]} />
                    </Field>
                    <Field data-invalid={!!formState.errors.models}>
                      <FieldLabel htmlFor="add-models">Models</FieldLabel>
                      <Input id="add-models" autoComplete="off" placeholder="model-a, model-b" aria-invalid={!!formState.errors.models} {...register('models')} />
                      <FieldDescription>Comma-separated model ids.</FieldDescription>
                      <FieldError errors={[formState.errors.models]} />
                    </Field>
                    {(['Opus', 'Sonnet', 'Haiku'] as const).map((slot) => {
                      const name = `slot${slot}` as 'slotOpus' | 'slotSonnet' | 'slotHaiku'
                      return (
                        <Field key={slot} data-invalid={!!formState.errors[name]}>
                          <FieldLabel htmlFor={`add-slot-${slot}`}>{slot} model</FieldLabel>
                          <Input id={`add-slot-${slot}`} autoComplete="off" aria-invalid={!!formState.errors[name]} {...register(name)} />
                          <FieldError errors={[formState.errors[name]]} />
                        </Field>
                      )
                    })}
                  </>
                ) : (
                  <Field>
                    <FieldLabel htmlFor="add-base-url">Base URL</FieldLabel>
                    <Input id="add-base-url" readOnly value={PRESET_URL[preset as keyof typeof PRESET_URL]} aria-invalid={!!formState.errors.baseUrl} />
                    <FieldError errors={[formState.errors.baseUrl]} />
                  </Field>
                )}
                <FieldSet>
                  <FieldLegend variant="label">API key</FieldLegend>
                  <Controller
                    control={control}
                    name="tokenSource"
                    render={({ field }) => (
                      <div role="radiogroup" aria-label="API key source" className="flex gap-4 text-sm">
                        {([['paste', 'Paste key'], ['env', 'Environment variable']] as const).map(([v, label]) => (
                          <label key={v} className="flex items-center gap-2">
                            <input type="radio" name="token-source" value={v} checked={(field.value ?? 'paste') === v} onChange={() => field.onChange(v)} />
                            {label}
                          </label>
                        ))}
                      </div>
                    )}
                  />
                  {tokenSource === 'env' ? (
                    <Field data-invalid={!!formState.errors.tokenEnv}>
                      <FieldLabel htmlFor="add-token-env">Variable name</FieldLabel>
                      <Input id="add-token-env" autoComplete="off" placeholder="ZAI_API_KEY" aria-invalid={!!formState.errors.tokenEnv} {...register('tokenEnv')} />
                      <FieldDescription>Read from the daemon environment at run time.</FieldDescription>
                      <FieldError errors={[formState.errors.tokenEnv]} />
                    </Field>
                  ) : (
                    <Field data-invalid={!!formState.errors.token}>
                      <FieldLabel htmlFor="add-token">API key</FieldLabel>
                      <Input id="add-token" type="password" autoComplete="off" aria-invalid={!!formState.errors.token} {...register('token')} />
                      <FieldDescription>Stored in a private file, never shown again.</FieldDescription>
                      <FieldError errors={[formState.errors.token]} />
                    </Field>
                  )}
                </FieldSet>
              </>
            )}
            <Collapsible open={advanced} onOpenChange={setAdvanced}>
              <CollapsibleTrigger asChild>
                <Button type="button" variant="ghost" size="sm" aria-expanded={advanced}>
                  <ChevronDown aria-hidden className={advanced ? 'rotate-180' : undefined} />
                  Advanced
                </Button>
              </CollapsibleTrigger>
              <CollapsibleContent className="pt-2">
                <Field data-invalid={!!formState.errors.home}>
                  <FieldLabel htmlFor="add-home">Credential home</FieldLabel>
                  <Input id="add-home" autoComplete="off" placeholder="default location" {...register('home')} />
                  <FieldDescription>Leave empty to use the default path. Only the path is stored, never its contents.</FieldDescription>
                  <FieldError errors={[formState.errors.home]} />
                </Field>
              </CollapsibleContent>
            </Collapsible>
          </FieldGroup>
          {blocked && (
            <p role="alert" className="text-sm text-destructive">
              Only one Antigravity account is supported on macOS, and one already exists.
            </p>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => changeOpen(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={create.isPending || blocked}>
              {create.isPending ? 'Adding…' : 'Add and sign in'}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

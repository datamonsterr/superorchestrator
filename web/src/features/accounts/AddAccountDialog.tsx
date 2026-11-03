import { useMemo, useState } from 'react'
import { Controller, useForm, useWatch } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { ChevronDown } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { useConfig, useCreateAccount } from '@/api/queries'
import { applyApiFieldErrors } from '@/lib/form-errors'
import type { AccountView } from '@/api/types'
import { EndpointFields } from './EndpointFields'
import { addAccountSchema, buildAddAccountRequest, HARNESSES, type AddAccountValues } from './schemas'

interface Props {
  open: boolean
  onOpenChange: (open: boolean) => void
  accounts: AccountView[]
  /** Called with the new account name so the page can open the sign-in step. */
  onCreated: (name: string) => void
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
  'endpoint.token_env': 'tokenEnv',
}
// No single input owns these, or the input may not be on screen: they go to a form-level message.
const FORM_LEVEL = new Set(['endpoint.slots'])
const API_ERROR_FIELDS = [...Object.keys(API_FIELD), ...FORM_LEVEL]

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

  async function onSubmit(values: AddAccountValues) {
    try {
      const body = buildAddAccountRequest(values)
      const account = await create.mutateAsync(body)
      changeOpen(false)
      onCreated(account.name)
    } catch (e) {
      applyApiFieldErrors(
        e,
        (name, error) => {
          const { tokenSource } = form.getValues()
          const hidden = (name === 'token' && tokenSource === 'env') || (name === 'endpoint.token_env' && tokenSource !== 'env')
          if (FORM_LEVEL.has(name) || hidden) setError('root.server', error)
          else setError(API_FIELD[name] ?? name, error)
        },
        API_ERROR_FIELDS,
      )
      // The token never stays in form state after a submit, even a failed one.
      form.setValue('token', '')
    } finally {
      // Drops the request (which holds the API key) from the mutation observer.
      create.reset()
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
                  <Select
                    value={field.value}
                    onValueChange={(v) => {
                      field.onChange(v)
                      form.setValue('token', '')
                    }}
                  >
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
            <EndpointFields form={form} />
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
          {formState.errors.root?.server?.message && (
            <p role="alert" className="text-sm text-destructive">
              {formState.errors.root.server.message}
            </p>
          )}
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

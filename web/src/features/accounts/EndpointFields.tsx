import { Controller, useWatch, type UseFormReturn } from 'react-hook-form'
import { Field, FieldDescription, FieldError, FieldLabel, FieldLegend, FieldSet } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { ENDPOINT_PRESETS, PRESET_INFO } from './endpoint-presets'
import type { AddAccountValues } from './schemas'

const SLOTS = [
  ['slotOpus', 'Opus model'],
  ['slotSonnet', 'Sonnet model'],
  ['slotHaiku', 'Haiku model'],
] as const

/** The Endpoint select (claude only) and, once a preset is picked, its connection and API key inputs. */
export function EndpointFields({ form }: { form: UseFormReturn<AddAccountValues> }) {
  const { control, register, setValue, formState } = form
  const { errors } = formState
  const harness = useWatch({ control, name: 'harness' })
  const preset = useWatch({ control, name: 'endpointPreset' }) ?? 'none'
  const tokenSource = useWatch({ control, name: 'tokenSource' }) ?? 'paste'
  // A key typed for one choice must never carry over to another.
  const clearToken = () => setValue('token', '')

  if (harness !== 'claude') return null
  const presetUrl = PRESET_INFO[preset].url
  return (
    <>
      <Field data-invalid={!!errors.endpointPreset}>
        <FieldLabel htmlFor="add-endpoint">Endpoint</FieldLabel>
        <Controller
          control={control}
          name="endpointPreset"
          render={({ field }) => (
            <Select
              value={field.value ?? 'none'}
              onValueChange={(v) => {
                field.onChange(v)
                clearToken()
              }}
            >
              <SelectTrigger id="add-endpoint" className="w-full" aria-invalid={!!errors.endpointPreset}>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {ENDPOINT_PRESETS.map((p) => (
                  <SelectItem key={p} value={p}>
                    {PRESET_INFO[p].label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}
        />
        <FieldError errors={[errors.endpointPreset]} />
      </Field>
      {preset !== 'none' && (
        <>
          {preset === 'custom' ? (
            <>
              <Field data-invalid={!!errors.baseUrl}>
                <FieldLabel htmlFor="add-base-url">Base URL</FieldLabel>
                <Input id="add-base-url" autoComplete="off" placeholder="https://api.example.com/anthropic" aria-invalid={!!errors.baseUrl} {...register('baseUrl')} />
                <FieldError errors={[errors.baseUrl]} />
              </Field>
              <Field data-invalid={!!errors.models}>
                <FieldLabel htmlFor="add-models">Models</FieldLabel>
                <Input id="add-models" autoComplete="off" placeholder="model-a, model-b" aria-invalid={!!errors.models} {...register('models')} />
                <FieldDescription>Comma-separated model ids.</FieldDescription>
                <FieldError errors={[errors.models]} />
              </Field>
              {SLOTS.map(([name, label]) => (
                <Field key={name} data-invalid={!!errors[name]}>
                  <FieldLabel htmlFor={`add-${name}`}>{label}</FieldLabel>
                  <Input id={`add-${name}`} autoComplete="off" aria-invalid={!!errors[name]} {...register(name)} />
                  <FieldError errors={[errors[name]]} />
                </Field>
              ))}
            </>
          ) : (
            <Field data-invalid={!!errors.baseUrl}>
              <FieldLabel htmlFor="add-base-url">Base URL</FieldLabel>
              <Input id="add-base-url" readOnly value={presetUrl ?? ''} aria-invalid={!!errors.baseUrl} />
              <FieldError errors={[errors.baseUrl]} />
            </Field>
          )}
          <FieldSet>
            <FieldLegend variant="label">API key source</FieldLegend>
            <Controller
              control={control}
              name="tokenSource"
              render={({ field }) => (
                <div className="flex gap-4 text-sm">
                  {(
                    [
                      ['paste', 'Paste key'],
                      ['env', 'Environment variable'],
                    ] as const
                  ).map(([v, label]) => (
                    <label key={v} className="flex items-center gap-2">
                      <input
                        type="radio"
                        name="token-source"
                        value={v}
                        checked={(field.value ?? 'paste') === v}
                        onChange={() => {
                          field.onChange(v)
                          clearToken()
                        }}
                      />
                      {label}
                    </label>
                  ))}
                </div>
              )}
            />
            {tokenSource === 'env' ? (
              <Field data-invalid={!!errors.tokenEnv}>
                <FieldLabel htmlFor="add-token-env">Variable name</FieldLabel>
                <Input id="add-token-env" autoComplete="off" placeholder="ZAI_API_KEY" aria-invalid={!!errors.tokenEnv} {...register('tokenEnv')} />
                <FieldDescription>Read from the daemon environment at run time.</FieldDescription>
                <FieldError errors={[errors.tokenEnv]} />
              </Field>
            ) : (
              <Field data-invalid={!!errors.token}>
                <FieldLabel htmlFor="add-token">API key</FieldLabel>
                <Input id="add-token" type="password" autoComplete="off" aria-invalid={!!errors.token} {...register('token')} />
                <FieldDescription>Stored in a private file, never shown again.</FieldDescription>
                <FieldError errors={[errors.token]} />
              </Field>
            )}
          </FieldSet>
        </>
      )}
    </>
  )
}

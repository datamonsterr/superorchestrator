import { Controller, useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Textarea } from '@/components/ui/textarea'
import { ErrorAlert } from '@/components/common/ErrorAlert'
import { HarnessBadge } from '@/components/common/HarnessBadge'
import { ProbabilityBars } from '@/components/common/ProbabilityBars'
import { useAccounts, useRoutePreview } from '@/api/queries'
import { formatMoney } from '@/lib/format'
import type { RouteResult } from '@/api/types'
import { ANY, HARNESSES, paramsFrom, previewSchema, type PreviewValues } from './schemas'

export function RouteResultCard({ result }: { result: RouteResult }) {
  return (
    <Card aria-label="Route result">
      <CardHeader>
        <CardTitle className="flex flex-wrap items-center gap-2 text-base">
          <span className="font-mono">{result.choice_id}</span>
          <span className="text-sm font-normal text-muted-foreground">on {result.account}</span>
          <Badge variant={result.source === 'pinned' ? 'secondary' : 'outline'}>source: {result.source}</Badge>
          {result.confidence != null && <Badge variant="outline">confidence {result.confidence.toFixed(2)}</Badge>}
        </CardTitle>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        <div className="flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
          <HarnessBadge harness={result.harness} />
          <span className="font-mono">{result.model}</span>
          {result.effort && <span>effort {result.effort}</span>}
          <span>routing cost {formatMoney(result.usage.cost)}</span>
        </div>
        <ProbabilityBars probabilities={result.probabilities} selected={result.choice_id} />
        <p className="text-sm">
          <span className="font-medium">Rationale: </span>
          {result.rationale}
        </p>
        {result.alternatives.length > 0 && (
          <p className="text-sm text-muted-foreground">Alternatives: {result.alternatives.map((a) => a.choice_id).join(', ')}</p>
        )}
      </CardContent>
    </Card>
  )
}

export function RoutePreview() {
  const preview = useRoutePreview()
  const accounts = useAccounts()
  const form = useForm<PreviewValues>({
    resolver: zodResolver(previewSchema),
    defaultValues: { prompt: '', role: '', harness: ANY, account: ANY, model: '', effort: '' },
  })
  const { control, register, handleSubmit, formState } = form

  return (
    <div className="grid gap-6 lg:grid-cols-[minmax(0,26rem)_1fr]">
      <form onSubmit={handleSubmit((v) => preview.mutate(paramsFrom(v)))} noValidate className="flex flex-col gap-4">
        <FieldGroup>
          <Field data-invalid={!!formState.errors.prompt}>
            <FieldLabel htmlFor="preview-prompt">Prompt</FieldLabel>
            <Textarea id="preview-prompt" rows={5} aria-invalid={!!formState.errors.prompt} {...register('prompt')} />
            <FieldError errors={[formState.errors.prompt]} />
          </Field>
          <Field>
            <FieldLabel htmlFor="preview-role">Role (optional)</FieldLabel>
            <Input id="preview-role" placeholder="worker" autoComplete="off" {...register('role')} />
          </Field>
          <fieldset className="flex flex-col gap-3 rounded-md border p-3">
            <legend className="px-1 text-sm font-medium">Pin</legend>
            <FieldDescription>Pinned fields skip the router for that part.</FieldDescription>
            <Field>
              <FieldLabel htmlFor="preview-harness">Harness</FieldLabel>
              <Controller
                control={control}
                name="harness"
                render={({ field }) => (
                  <Select value={field.value} onValueChange={field.onChange}>
                    <SelectTrigger id="preview-harness" className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={ANY}>Any</SelectItem>
                      {HARNESSES.map((h) => (
                        <SelectItem key={h} value={h}>
                          {h}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              />
            </Field>
            <Field>
              <FieldLabel htmlFor="preview-account">Account</FieldLabel>
              <Controller
                control={control}
                name="account"
                render={({ field }) => (
                  <Select value={field.value} onValueChange={field.onChange}>
                    <SelectTrigger id="preview-account" className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={ANY}>Any</SelectItem>
                      {(accounts.data ?? []).map((a) => (
                        <SelectItem key={a.name} value={a.name}>
                          {a.name}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                )}
              />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field>
                <FieldLabel htmlFor="preview-model">Model</FieldLabel>
                <Input id="preview-model" autoComplete="off" {...register('model')} />
              </Field>
              <Field>
                <FieldLabel htmlFor="preview-effort">Effort</FieldLabel>
                <Input id="preview-effort" autoComplete="off" {...register('effort')} />
              </Field>
            </div>
          </fieldset>
        </FieldGroup>
        <Button type="submit" disabled={preview.isPending} className="self-start">
          {preview.isPending ? 'Routing…' : 'Run preview'}
        </Button>
      </form>
      <div aria-live="polite" className="min-w-0">
        {preview.isError ? (
          <ErrorAlert error={preview.error} title="Route failed" />
        ) : preview.data ? (
          <RouteResultCard result={preview.data} />
        ) : (
          <p className="text-sm text-muted-foreground">The preview asks the router, which may cost a little. Nothing is started.</p>
        )}
      </div>
    </div>
  )
}

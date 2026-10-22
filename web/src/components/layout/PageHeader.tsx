import type { ReactNode } from 'react'

interface Props {
  title: string
  description?: ReactNode
  actions?: ReactNode
}

/** The only h1 of a page. */
export function PageHeader({ title, description, actions }: Props) {
  return (
    <div className="mb-4 flex flex-wrap items-start justify-between gap-2">
      <div className="min-w-0">
        <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
        {description && <p className="text-sm text-muted-foreground">{description}</p>}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </div>
  )
}

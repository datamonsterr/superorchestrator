import { useEffect, useRef, useState } from 'react'
import { Check, Copy } from 'lucide-react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'

interface Props {
  value: string
  label?: string
  className?: string
  /** Toast text; pass null to stay silent. */
  toastText?: string | null
}

/** Copies via the async clipboard. Without a secure context it fails quietly (design open question 6). */
export function CopyButton({ value, label = 'Copy', className, toastText = 'Copied' }: Props) {
  const [copied, setCopied] = useState(false)
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  useEffect(() => () => clearTimeout(timer.current), [])

  async function copy() {
    try {
      await navigator.clipboard.writeText(value)
    } catch {
      return // insecure context or denied: the text stays selectable
    }
    setCopied(true)
    if (toastText) toast.success(toastText)
    clearTimeout(timer.current)
    timer.current = setTimeout(() => setCopied(false), 1500)
  }

  return (
    <>
      <Button type="button" variant="outline" size="sm" className={className} onClick={copy} aria-label={label}>
        {copied ? <Check aria-hidden /> : <Copy aria-hidden />}
        <span aria-hidden>{label}</span>
      </Button>
      <span role="status" className="sr-only">
        {copied ? 'Copied' : ''}
      </span>
    </>
  )
}

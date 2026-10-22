export interface Frontmatter {
  /** Top-level `key: value` pairs of the leading `---` block; `null` when there is none. */
  fields: Record<string, string> | null
  /** Local hints only. The server is the authority and its 422 message is shown after save. */
  hints: string[]
}

export const NEW_AGENT_TEMPLATE = `---
name: my-agent
description: What this agent is for
---
Instructions for the agent go here.
`

/** Cheap client-side look at the frontmatter, for live feedback while typing. */
export function readFrontmatter(source: string): Frontmatter {
  const m = /^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/.exec(source)
  if (!m) return { fields: null, hints: ['The file must start with a `---` frontmatter block.'] }
  const fields: Record<string, string> = {}
  for (const line of m[1].split(/\r?\n/)) {
    const kv = /^([A-Za-z_][\w-]*):\s*(.*)$/.exec(line)
    if (kv) fields[kv[1]] = kv[2].trim()
  }
  const hints: string[] = []
  if (!fields.name) hints.push('The frontmatter needs a `name`.')
  if (!fields.description) hints.push('The frontmatter needs a `description`.')
  return { fields, hints }
}

import { describe, expect, it } from 'vitest'
import { NEW_AGENT_TEMPLATE, readFrontmatter } from './frontmatter'

describe('readFrontmatter', () => {
  it('parses name and description and has no hints for the template', () => {
    const f = readFrontmatter(NEW_AGENT_TEMPLATE)
    expect(f.fields).toMatchObject({ name: 'my-agent', description: 'What this agent is for' })
    expect(f.hints).toEqual([])
  })
  it('hints at a missing block and missing fields', () => {
    expect(readFrontmatter('hello').fields).toBeNull()
    expect(readFrontmatter('hello').hints).toHaveLength(1)
    expect(readFrontmatter('---\nname: x\n---\nbody').hints).toEqual(['The frontmatter needs a `description`.'])
  })
})

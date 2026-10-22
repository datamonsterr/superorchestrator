---
name: use-otherskill
description: Use the OtherSkill CLI to configure harness hooks and OpenRouter routing, add categorized skills, select skills for a prompt, and edit skill-set settings.
---

# Use OtherSkill

Use `otherskill --help` and the relevant subcommand's help for installed-version flags. The four main commands are `setup`, `add`, `find`, and `config`.

## Setup

Run `otherskill setup` for the interactive menu. Choose the harness (Claude, Codex, Antigravity, OpenCode, or another supported adapter), global or project hook scope, OpenRouter API credentials, a free decision model, and enabled categories starting from the shipped defaults. Use documented noninteractive flags and dry-run mode for scripted setup.

Keep the categorized bank outside native skill discovery. Only this companion is installed natively: Claude uses `.claude/skills/use-otherskill`; other supported harnesses use `.agents/skills/use-otherskill`. Preserve unrelated hooks, configuration, and native skills. Never print or commit API credentials.

## Add

Use `otherskill add` with a supported local or online skill source. Inspect source, capability, and license before choosing a skill. The decision model assigns it to matching configured categories. Installation copies the entire ordinary skill directory into every match, including scripts, references, assets, and source/version attribution. Do not substitute tags or expose category symlinks natively.

For locally authored skills, write a concise name and description, focused instructions, and supporting files only when needed. State when the skill applies. Preserve intended behavior and attribution when updating an imported skill, then add/reclassify it so category copies stay consistent. Management does not authorize publishing or removing unrelated skills.

## Find

Use `otherskill find --agent <harness> --prompt <user prompt>` to select skills for the task. This is also the hook entrypoint. Routing first selects relevant categories, then sends concurrent OpenRouter decision requests within each selected category using the original prompt and all candidate descriptions. It returns matching skills; the native hook adds only the selected descriptions to agent context.

Pass prompt text as a literal argument through the tool's argument interface or proper shell quoting; never build an executable shell string from user text. Installed hooks may accept native payloads through stdin as documented. Do not scan the hidden bank or read every installed skill instead of routing.

## Config

Run `otherskill config` to inspect and quickly edit/apply settings interactively. Use its documented show/get/set interface for scripts. Check which project or global config is effective before changing the model, enabled categories, bank, or harness settings.

Use a configured free OpenRouter decision model. Do not replace a failed free request with a paid model. Distinguish no matches from provider, quota, or configuration failures using diagnostics; report unavailable evidence as unavailable. Keep credentials and user prompts out of diagnostic output.

---
name: security
description: Project security policies, vulnerability reporting, and credential hygiene. Use when auditing credentials, handling leaked API keys, managing secrets, or reviewing security compliance.
---

# Project Security & Credential Hygiene

This skill defines the mandatory security policies, secret protection workflows, and emergency credential rotation procedures.

## Quick Reference: Secret Protection Rules

1. **Environment Variables**:
   - Never commit live secrets or real tokens into `.env.example` or any version-controlled file.
   - All private keys belong in `.env` (guaranteed by `.gitignore`).
   - Treat `AI_LOG_API_KEY` and LLM provider keys as confidential.

2. **Pre-Commit Secret Audit**:
   Always run a quick scan before committing:
   ```bash
   git diff --cached | grep -iE "api[_-]?key|secret|token|password"
   ```

3. **Prompt Log Auditing**:
   The `.ai-log/` directory tracks raw prompts. Verify no sensitive personal or customer data exists in `.ai-log/session.jsonl` before publishing or sharing repository forks.

## Incident Response: Accidental Key Leak

If a secret or API key was accidentally committed to git:

> **IMPORTANT**: Simply removing the key in a subsequent commit is **INSUFFICIENT**. The key remains completely exposed in git commit history and existing clones.

### Mandatory Remediation Steps:

1. **Rotate Immediately (Non-negotiable)**:
   - Invalidate and regenerate the compromised key in the provider console (OpenAI, Phoenix, Anthropic, etc.).
   - Updating the provider key is the only action that guarantees access termination.

2. **Clean Working Tree**:
   - Replace the key with a placeholder `<REPLACED>` and save the file.

3. **Purge from Git History**:
   - Use `git-filter-repo` to completely excise the secret from all branches and tags:
     ```bash
     pip install git-filter-repo
     git filter-repo --replace-text <(echo 'compromised_secret_string==>REDACTED')
     git push origin --force --all
     ```

4. **Notify Maintainers**:
   - Inform repository maintainers and team members so they can re-clone or rebase their local working trees.

## Vulnerability Reporting

- Never open a public issue for suspected vulnerabilities.
- Report security advisories privately via GitHub Security Advisories or maintainer email.

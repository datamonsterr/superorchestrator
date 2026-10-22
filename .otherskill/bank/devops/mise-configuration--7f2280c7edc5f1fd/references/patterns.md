# mise [env] Code Patterns

## Table of Contents

- [Python Venv Auto-Creation](#python-venv-auto-creation)
  - [Basic Pattern](#basic-pattern)
  - [With uv Auto-Venv via Settings](#with-uv-auto-venv-via-settings)
  - [Project Template with Venv](#project-template-with-venv)
- [Special Directives](#special-directives)
  - [Load from .env Files (`_.file`)](#load-from-env-files-_file)
  - [Extend PATH (`_.path`)](#extend-path-_path)
  - [Source Bash Scripts (`_.source`)](#source-bash-scripts-_source)
  - [Complete Special Directives Example](#complete-special-directives-example)
- [Template Syntax (Tera)](#template-syntax-tera)
  - [Built-in Variables](#built-in-variables)
  - [Functions](#functions)
  - [Filters](#filters)
  - [Conditionals](#conditionals)
  - [Complete Template Example](#complete-template-example)
- [Required & Redacted Variables](#required--redacted-variables)
  - [Required Variables](#required-variables)
  - [Redacted Variables](#redacted-variables)
  - [Combined Patterns](#combined-patterns)
- [[settings] Section](#settings-section)
  - [Python Development Setup](#python-development-setup)
- [[tools] Version Pinning](#tools-version-pinning)
  - [Basic Pinning](#basic-pinning)
  - [With Options](#with-options)
  - [min_version Enforcement](#min_version-enforcement)
  - [Full Development Environment](#full-development-environment)
- [Python Pattern](#python-pattern)
- [Bash Pattern](#bash-pattern)
- [JavaScript/Node.js Pattern](#javascriptnodejs-pattern)
- [Go Pattern](#go-pattern)
- [Rust Pattern](#rust-pattern)
- [Complete .mise.toml Template](#complete-misetoml-template)
- [Real-World Examples](#real-world-examples)
- [Testing Pattern](#testing-pattern)
- [Migration Checklist](#migration-checklist)

Complete code patterns for implementing mise `[env]` configuration with backward-compatible defaults.

## Python Venv Auto-Creation

The most critical mise pattern - auto-create and activate Python virtual environments:

### Basic Pattern

```toml
# .mise.toml
[env]
_.python.venv = { path = ".venv", create = true }
```

**What it does:**

1. Creates `.venv` if it doesn't exist when entering directory
2. Automatically activates the venv
3. Works with `uv` for fast venv creation

### With uv Auto-Venv via Settings

```toml
# .mise.toml
[settings]
python.uv_venv_auto = true

[tools]
python = "3.11"  # baseline >=3.11; pin to project needs
uv = "latest"
```

### Project Template with Venv

```toml
# .mise.toml - Python project with auto-venv
[env]
_.python.venv = { path = ".venv", create = true }
PYTHONUNBUFFERED = "1"
PYTHONDONTWRITEBYTECODE = "1"

[tools]
python = "3.11"  # baseline >=3.11; pin to project needs
uv = "latest"
```

## Special Directives

### Load from .env Files (`_.file`)

```toml
[env]
# Single .env file
_.file = ".env"

# Multiple files with options
_.file = [
    ".env",
    ".env.local",
    { path = ".env.secrets", redact = true }
]

# Load after tools are installed
_.file = { path = ".env", tools = true }
```

**Use case:** Load existing .env files without duplicating values in .mise.toml.

### Extend PATH (`_.path`)

```toml
[env]
# Add project directories to PATH
_.path = [
    "{{config_root}}/bin",
    "{{config_root}}/scripts",
    "node_modules/.bin"
]
```

**Use case:** Make project scripts and tool binaries available without full path.

### Source Bash Scripts (`_.source`)

```toml
[env]
# Simple script
_.source = "./scripts/env.sh"

# With secret redaction
_.source = { path = ".secrets.sh", redact = true }
```

**Use case:** Complex environment setup that requires bash logic.

### Complete Special Directives Example

```toml
# .mise.toml - Full-featured project setup
[env]
# 1. Auto-create Python venv
_.python.venv = { path = ".venv", create = true }

# 2. Load .env files
_.file = [
    ".env",
    { path = ".env.local", redact = true }
]

# 3. Extend PATH
_.path = [
    "{{config_root}}/bin",
    "{{config_root}}/scripts"
]

# 4. Project configuration
PROJECT_NAME = "my-project"
LOG_LEVEL = "info"
```

## Template Syntax (Tera)

mise uses Tera templating engine. Reference for common patterns:

### Built-in Variables

```toml
[env]
# Directory paths
PROJECT_ROOT = "{{config_root}}"        # .mise.toml directory
CURRENT_DIR = "{{cwd}}"                 # Current working directory

# XDG directories
CACHE = "{{xdg_cache_home}}/myapp"
CONFIG = "{{xdg_config_home}}/myapp"
DATA = "{{xdg_data_home}}/myapp"

# mise info
MISE_BIN = "{{mise_bin}}"
MISE_PID = "{{mise_pid}}"
```

### Functions

```toml
[env]
# Get env var with fallback
NODE_VER = "{{ get_env(name='NODE_VERSION', default='20') }}"

# Execute shell command
BUILD_TIME = "{{ exec(command='date +%Y-%m-%d') }}"
GIT_SHA = "{{ exec(command='git rev-parse --short HEAD') }}"

# System info
ARCH = "{{ arch() }}"           # x64, arm64
OS = "{{ os() }}"               # linux, macos, windows
CPUS = "{{ num_cpus() }}"
OS_FAMILY = "{{ os_family() }}" # unix, windows

# File operations
VERSION = "{{ read_file(path='VERSION') | trim }}"
CONFIG_HASH = "{{ hash_file(path='config.json', len=8) }}"

# Directory check
{% if is_dir("src") %}
SRC_EXISTS = "true"
{% endif %}
```

### Filters

```toml
[env]
# Case conversion
SNAKE_NAME = "{{ project_name | snakecase }}"    # my_project
KEBAB_NAME = "{{ project_name | kebabcase }}"    # my-project
CAMEL_NAME = "{{ project_name | lowercamelcase }}" # myProject
PASCAL_NAME = "{{ project_name | uppercamelcase }}" # MyProject

# String manipulation
CLEAN = "{{ raw_value | trim }}"
UPPER = "{{ name | upper }}"
LOWER = "{{ name | lower }}"
REPLACED = "{{ text | replace(from='-', to='_') }}"

# Path operations
ABS_PATH = "{{ relative_path | absolute }}"
FILE_NAME = "{{ full_path | basename }}"
DIR_NAME = "{{ full_path | dirname }}"
FILE_STEM = "{{ full_path | file_stem }}"         # without extension
EXTENSION = "{{ full_path | file_extension }}"

# String utilities
QUOTED = "{{ value | quote }}"
LAST_ITEM = "{{ list | last }}"
FIRST_ITEM = "{{ list | first }}"
```

### Conditionals

```toml
[env]
{% if env.CI %}
# CI-specific settings
LOG_LEVEL = "error"
PARALLEL = "{{ num_cpus() }}"
{% else %}
# Local development
LOG_LEVEL = "debug"
PARALLEL = "2"
{% endif %}

{% if os() == "macos" %}
BREW_PREFIX = "/opt/homebrew"
{% elif os() == "linux" %}
BREW_PREFIX = "/home/linuxbrew/.linuxbrew"
{% endif %}
```

### Complete Template Example

```toml
# .mise.toml - Template-heavy configuration
[env]
# Computed paths
PROJECT_ROOT = "{{config_root}}"
BUILD_DIR = "{{config_root}}/build/{{ os() }}-{{ arch() }}"
CACHE_DIR = "{{xdg_cache_home}}/{{ cwd | basename }}"

# Git-derived values
GIT_BRANCH = "{{ exec(command='git branch --show-current') | trim }}"
GIT_SHA = "{{ exec(command='git rev-parse --short HEAD') | trim }}"
VERSION = "{{ read_file(path='VERSION') | trim | default(value='0.0.0') }}"

# Platform-specific
{% if os() == "macos" %}
DYLD_LIBRARY_PATH = "{{config_root}}/lib"
{% else %}
LD_LIBRARY_PATH = "{{config_root}}/lib"
{% endif %}

# Environment-aware
{% if get_env(name='CI', default='false') == 'true' %}
LOG_LEVEL = "error"
PARALLEL_JOBS = "{{ num_cpus() }}"
{% else %}
LOG_LEVEL = "debug"
PARALLEL_JOBS = "4"
{% endif %}
```

## Required & Redacted Variables

### Required Variables

```toml
[env]
# Simple required - fails if not set
DATABASE_URL = { required = true }

# Required with help message
API_KEY = { required = "Get your API key from https://example.com/settings" }
GITHUB_TOKEN = { required = "Run: gh auth token" }
```

**Behavior:** mise shows error and refuses to activate if required variable is unset.

### Redacted Variables

```toml
[env]
# Redact specific variable
SECRET_KEY = { value = "{{ exec(command='op read op://vault/item/password') }}", redact = true }

# Redact entire .env file
_.file = { path = ".env.secrets", redact = true }

# Pattern-based redactions (hides in `mise env` output)
redactions = ["*_TOKEN", "*_KEY", "*_SECRET", "PASSWORD", "CREDENTIAL"]
```

### Combined Patterns

```toml
[env]
# Public configuration
LOG_LEVEL = "info"
OUTPUT_DIR = "output"

# Required with help
DOPPLER_PROJECT = { required = "Set your Doppler project name" }

# Secrets from external sources (redacted)
API_KEY = { value = "{{ exec(command='doppler secrets get API_KEY --plain') }}", redact = true }

# Pattern-based redaction for anything else
redactions = ["*_TOKEN", "*_KEY"]
```

## [settings] Section

Configure mise behavior:

```toml
[settings]
# Enable experimental features
experimental = true

# Python-specific
python.uv_venv_auto = true           # Auto-create venv with uv
python.default_packages_file = ".default-python-packages"

# Node.js-specific
node.default_packages_file = ".default-npm-packages"

# Task runner
task.auto_install = true              # Auto-install task dependencies

# General
always_keep_download = false
always_keep_install = false
verbose = false
```

### Python Development Setup

```toml
[settings]
experimental = true
python.uv_venv_auto = true

[tools]
python = "3.11"  # baseline >=3.11; pin to project needs
uv = "latest"

[env]
PYTHONUNBUFFERED = "1"
```

## [tools] Version Pinning

Pin tool versions for reproducibility:

### Basic Pinning

```toml
[tools]
python = "3.11"  # baseline >=3.11; pin to project needs
node = "latest"
uv = "latest"
rust = "1.75"
```

### With Options

```toml
[tools]
# Specific version
python = "3.12.3"

# Version prefix (latest 3.12.x)
python = "3.11"  # baseline >=3.11; pin to project needs

# Latest
uv = "latest"

# With backend options
rust = { version = "1.75", profile = "minimal" }

# Multiple versions (first is default)
node = ["22", "20", "18"]
```

### min_version Enforcement

```toml
# Require minimum mise version
min_version = "2024.9.5"

[tools]
python = "3.11"  # baseline >=3.11; pin to project needs
```

### Full Development Environment

```toml
# .mise.toml - Complete development environment
min_version = "2024.9.5"

[settings]
experimental = true
python.uv_venv_auto = true

[tools]
python = "3.11"  # baseline >=3.11; pin to project needs
node = "latest"
uv = "latest"
rust = "1.75"

[env]
_.python.venv = { path = ".venv", create = true }
_.path = ["{{config_root}}/bin", "node_modules/.bin"]

PYTHONUNBUFFERED = "1"
NODE_ENV = "development"
```

## Python Pattern

```python
#!/usr/bin/env python3
"""Example script with mise [env] configuration."""

import os

# ADR: 2025-12-08-mise-env-centralized-config
# Configuration from environment with defaults
TIMEOUT = int(os.environ.get("SCRIPT_TIMEOUT", "300"))
OUTPUT_DIR = os.environ.get("OUTPUT_DIR", "output")
PARALLEL_WORKERS = int(os.environ.get("PARALLEL_WORKERS", "4"))
DEBUG_MODE = os.environ.get("DEBUG_MODE", "false").lower() == "true"

def main():
    print(f"Running with timeout={TIMEOUT}, workers={PARALLEL_WORKERS}")
    # ... script logic

if __name__ == "__main__":
    main()
```

**Key points:**

- Import `os` at top
- Define constants immediately after imports
- Cast to int/bool as needed (env vars are always strings)
- Use descriptive variable names matching .mise.toml

## Bash Pattern

```bash
/usr/bin/env bash << 'CONFIG_EOF'
#!/usr/bin/env bash
set -euo pipefail

# ADR: 2025-12-08-mise-env-centralized-config
# Configuration from environment with defaults
SCRIPT_TIMEOUT="${SCRIPT_TIMEOUT:-300}"
OUTPUT_DIR="${OUTPUT_DIR:-output}"
PARALLEL_WORKERS="${PARALLEL_WORKERS:-4}"
DEBUG_MODE="${DEBUG_MODE:-false}"

main() {
    echo "Running with timeout=$SCRIPT_TIMEOUT, workers=$PARALLEL_WORKERS"
    # ... script logic
}

main "$@"
CONFIG_EOF
```

**Key points:**

- Use `${VAR:-default}` POSIX syntax
- Define after shebang and set options
- No export needed - variables are local to script
- For boolean checks: `[[ "$DEBUG_MODE" == "true" ]]`

## JavaScript/Node.js Pattern

```javascript
#!/usr/bin/env node
/**
 * Example script with mise [env] configuration.
 */

// ADR: 2025-12-08-mise-env-centralized-config
// Configuration from environment with defaults
const TIMEOUT = parseInt(process.env.SCRIPT_TIMEOUT || "300", 10);
const OUTPUT_DIR = process.env.OUTPUT_DIR || "output";
const PARALLEL_WORKERS = parseInt(process.env.PARALLEL_WORKERS || "4", 10);
const DEBUG_MODE = process.env.DEBUG_MODE === "true";

async function main() {
  console.log(`Running with timeout=${TIMEOUT}, workers=${PARALLEL_WORKERS}`);
  // ... script logic
}

main().catch(console.error);
```

**Key points:**

- Use `process.env.VAR || "default"` pattern
- parseInt with radix 10 for numbers
- Boolean: strict equality check `=== "true"`
- Watch for falsy "0" - use `?? "default"` if "0" is valid

## Go Pattern

```go
package main

import (
    "fmt"
    "os"
    "strconv"
)

// ADR: 2025-12-08-mise-env-centralized-config
func getEnv(key, defaultValue string) string {
    if value := os.Getenv(key); value != "" {
        return value
    }
    return defaultValue
}

func getEnvInt(key string, defaultValue int) int {
    if value := os.Getenv(key); value != "" {
        if i, err := strconv.Atoi(value); err == nil {
            return i
        }
    }
    return defaultValue
}

var (
    Timeout         = getEnvInt("SCRIPT_TIMEOUT", 300)
    OutputDir       = getEnv("OUTPUT_DIR", "output")
    ParallelWorkers = getEnvInt("PARALLEL_WORKERS", 4)
)

func main() {
    fmt.Printf("Running with timeout=%d, workers=%d\n", Timeout, ParallelWorkers)
}
```

## Rust Pattern

```rust
use std::env;

// ADR: 2025-12-08-mise-env-centralized-config
fn get_env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

fn get_env_int(key: &str, default: i32) -> i32 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let timeout = get_env_int("SCRIPT_TIMEOUT", 300);
    let output_dir = get_env_or("OUTPUT_DIR", "output");
    let workers = get_env_int("PARALLEL_WORKERS", 4);

    println!("Running with timeout={}, workers={}", timeout, workers);
}
```

## Complete .mise.toml Template

```toml
# .mise.toml - Centralized configuration for this skill/project
# Values auto-load when shell has `mise activate` configured
# Scripts MUST work without mise (use defaults)

# Enforce minimum mise version for compatibility
min_version = "2024.9.5"

# ==============================================================================
# SETTINGS - mise behavior configuration
# ==============================================================================
[settings]
experimental = true
python.uv_venv_auto = true

# ==============================================================================
# TOOLS - Version pinning for reproducibility
# ==============================================================================
[tools]
python = "3.11"  # baseline >=3.11; pin to project needs
node = "latest"
uv = "latest"

# ==============================================================================
# ENVIRONMENT CONFIGURATION
# ==============================================================================
[env]
# --- Special Directives ---
# Auto-create Python venv
_.python.venv = { path = ".venv", create = true }

# Load .env files (optional)
# _.file = [".env", { path = ".env.local", redact = true }]

# Extend PATH with project binaries
_.path = ["{{config_root}}/bin", "{{config_root}}/scripts"]

# --- Project Paths ---
PROJECT_ROOT = "{{config_root}}"
OUTPUT_DIR = "output"
ADR_DIR = "docs/adr"
DESIGN_DIR = "docs/design"

# --- Timeouts (seconds) ---
SCRIPT_TIMEOUT = "300"
JSCPD_TIMEOUT = "120"

# --- Performance ---
PARALLEL_WORKERS = "4"

# --- Feature Flags ---
DEBUG_MODE = "false"
VERBOSE = "false"

# --- Python ---
PYTHONUNBUFFERED = "1"

# --- External Services (non-secrets only) ---
DOPPLER_PROJECT = "my-project"
DOPPLER_CONFIG = "prd"

# --- Redaction patterns for sensitive values ---
redactions = ["*_TOKEN", "*_KEY", "*_SECRET"]

# ==============================================================================
# TASKS - See mise-tasks skill for comprehensive task orchestration
# ==============================================================================
# [tasks]
# For task definitions with dependencies, arguments, and file tracking,
# invoke the mise-tasks skill: ../mise-tasks/SKILL.md
#
# Example tasks (uncomment and customize):
# [tasks.test]
# description = "Run test suite"
# run = "pytest tests/"
#
# [tasks.lint]
# description = "Run linters"
# run = "ruff check . && ruff format --check ."
#
# [tasks.build]
# description = "Build package"
# depends = ["lint", "test"]
# run = "uv build"
```

## Real-World Examples

### code-hardcode-audit/.mise.toml

```toml
[env]
AUDIT_PARALLEL_WORKERS = "4"
AUDIT_JSCPD_TIMEOUT = "300"
AUDIT_GITLEAKS_TIMEOUT = "120"
AUDIT_OUTPUT_FORMAT = "both"
PYTHONUNBUFFERED = "1"
```

### pypi-doppler/.mise.toml

```toml
[env]
DOPPLER_PROJECT = "claude-config"
DOPPLER_CONFIG = "prd"
DOPPLER_PYPI_SECRET = "PYPI_TOKEN"
PYPI_VERIFY_DELAY = "3"
```

### implement-plan-preflight/.mise.toml

```toml
[env]
ADR_DIR = "docs/adr"
DESIGN_DIR = "docs/design"
DESIGN_SPEC_FILENAME = "spec.md"
PREFLIGHT_STRICT_MODE = "true"
```

## Testing Pattern

```bash
# Test 1: Without mise (uses defaults)
unset SCRIPT_TIMEOUT OUTPUT_DIR
./script.py  # Should work with defaults

# Test 2: With mise activated
cd /path/to/skill
mise trust .mise.toml  # First time only
# Values auto-load from .mise.toml
./script.py  # Uses mise values

# Test 3: Override specific value
SCRIPT_TIMEOUT=60 ./script.py  # Explicit override wins
```

## Migration Checklist

When refactoring existing scripts to use mise `[env]`:

- [ ] Identify all hardcoded values (grep for magic numbers, paths)
- [ ] Create `.mise.toml` with `[env]` section
- [ ] Update script: add `os.environ.get()` with original as default
- [ ] Add ADR reference comment at config section
- [ ] Test: unset env vars, verify defaults work
- [ ] Test: set env vars manually, verify override works
- [ ] Test: in mise-activated shell, verify .mise.toml values load
- [ ] Document variables in skill's SKILL.md

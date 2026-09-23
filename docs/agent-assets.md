# Agent Assets

DevForge's AI agent is extensible through six families of **agent assets**:

| Family        | Folder (under a config root) | File format        | Injected into the prompt |
| ------------- | ---------------------------- | ------------------ | ------------------------ |
| **Skills**    | `skills/`                    | `*.md` / `SKILL.md` | yes                      |
| **MCPs**      | `mcp/`                       | `*.json`           | no (runtime servers)     |
| **Subagents** | `agents/`                    | `*.md`             | yes                      |
| **Rules**     | `rules/`                     | `*.md`             | yes                      |
| **Commands**  | `commands/`                  | `*.md`             | no (reusable workflows)  |
| **Hooks**     | root of the config root      | `hooks.json`       | no (runtime callbacks)   |

Ready-to-use examples live in [`defaults/agent-assets/`](../defaults/agent-assets/README.md).

## Managing assets in the UI

Open **Agent Assets** from any of these entry points:

- `Settings ▸ Open Agent Assets`
- The ✦ button in the AI composer toolbar
- The command palette (`Ctrl/Cmd+Shift+P`) → *Open Agent Assets*

The page mirrors Cursor's settings layout: a filter row
(`All | MCPs | Skills | Subagents | Rules | Commands | Hooks`), a
scope switch (**Project** / **User**), and a toolbar with three actions:

| Action             | What it does                                                              |
| ------------------ | ------------------------------------------------------------------------- |
| **New**            | Creates `<name>.md` (or `hooks.json` for Hooks) from a starter template.   |
| **Import file**    | Copies a single `.md` / `.json` file into the assets folder.               |
| **Import folder**  | Recursively imports every matching file from a folder (bundles, packs).    |
| **Run hooks**      | Opt-in switch for executing `hooks.json` commands (see [Hooks](#hooks)).    |

Each card shows the asset name, description, scope, on-disk path and an
**enabled** toggle. Toggling rewrites only the `enabled:` frontmatter line, so
comments and formatting in the rest of the file are preserved. Cards also expose
**Open** (edit in the editor) and **Delete** actions.

The toolbar also carries a **Run hooks** switch, which writes `ai.hooks-enabled`
to `settings.toml` and reloads the config immediately.

The selected tab also renders inline documentation and a working example of that
asset format, so you can copy the shape without leaving the editor.

## Where assets are discovered

Four roots are scanned, project first (a project asset shadows a user asset with
the same name):

| Scope       | Roots                                            |
| ----------- | ------------------------------------------------ |
| **Project** | `<workspace>/.devforge`, `<workspace>/.cursor`   |
| **User**    | `~/.devforge`, `~/.cursor`                       |

`.cursor` is read for compatibility, so skills and rules authored for Cursor keep
working. New assets are always written under `.devforge`.

## Asset formats

### Skills

A skill teaches the agent *how* to do something, and is applied only when its
`description` matches the task.

Two layouts are supported:

```
.devforge/skills/my-skill.md              # single file
.devforge/skills/my-skill/SKILL.md        # folder layout (Cursor style)
```

```markdown
---
name: rust-clippy-clean
description: >-
  Use when writing or reviewing Rust code. Keeps the workspace warning-free.
---

# Rust Clippy Clean

1. Run `cargo check` after every edit.
2. Run `cargo clippy --all-targets` and resolve new warnings.
```

For the folder layout the folder name is used when `name:` is omitted.

### MCPs

An MCP entry declares a stdio Model Context Protocol server. DevForge merges
these with the `[[ai.mcp-servers]]` entries from `settings.toml`; servers with the
same name are de-duplicated.

```json
{
  "name": "filesystem",
  "command": "npx",
  "args": ["-y", "@modelcontextprotocol/server-filesystem", "."],
  "enabled": true
}
```

`name`, `command` and `args` are required; `enabled` defaults to `true`.
Servers are only started when `ai.mcp-enabled = true` in settings.

### Subagents

A subagent is a focused assistant the main agent can delegate to. The markdown
body becomes that subagent's system prompt.

```markdown
---
name: code-reviewer
description: Use when a change is ready for a second pass.
---

You are a strict but constructive code reviewer. List findings by severity…
```

### Rules

Rules are **always** appended to the agent prompt, so keep them short and
concrete. Setting `enabled: false` parks a rule without deleting it.

```markdown
---
name: rust-style
description: Always applied Rust style rules for this workspace
enabled: true
---

- Never `unwrap()` on I/O or user input.
- Public items need a doc comment explaining *why*.
```

### Commands

A command packages a repeatable workflow you can trigger from chat.

```markdown
---
name: run-tests
description: Run the workspace test suite and summarise failures
---

1. `cargo fmt --check`
2. `cargo test -p devforge-agent --lib`
3. Report a table: crate | passed | failed | ignored.
```

### Hooks

Hooks declare shell commands bound to agent lifecycle events. They live in a
single `hooks.json` at the root of a config root (`.devforge/hooks.json`).

```json
{
  "version": 1,
  "hooks": {
    "afterFileEdit": [
      { "command": "cargo", "args": ["fmt", "--", "--check"] }
    ],
    "afterAgentRun": [
      { "command": "cargo", "args": ["test", "-p", "devforge-agent", "--lib"] }
    ]
  }
}
```

> **Status:** hooks are parsed, listed and editable today, and the runner is
> wired to the agent loop. Because hooks execute **local shell commands**, they
> are opt-in: flip the **Run hooks** switch on the Agent Assets toolbar, or set
> `ai.hooks-enabled = true` in `settings.toml`. While it is off the bundles are
> still listed in the UI, they just never spawn a process.

#### Supported events

| Event            | Fires when                                              |
| ---------------- | ------------------------------------------------------- |
| `afterFileEdit`  | a `write_file` / `str_replace` tool call succeeds        |
| `afterAgentRun`  | a run finishes (final answer, error, or max iterations)  |

Unknown event keys are ignored, so a bundle written for a newer DevForge keeps
working. Hooks run with the workspace root as their working directory, with a
60-second timeout each, and a failing hook never aborts the remaining hooks or
the agent run — its output is reported as an activity line in the chat.

## How assets reach the agent

1. On every run, `devforge_agent::assets::discover_all` scans the four roots.
2. Enabled **skills**, **subagents** and **rules** are rendered into the system
   prompt as `# Agent Skills`, `# Available Subagents` and `# Project Rules`.
3. Enabled **MCP** descriptors are merged into the MCP hub so their tools become
   callable (requires `ai.mcp-enabled = true`).
4. **Commands** are surfaced in the UI and via `list_agent_assets` so the agent
   can see what is installed; they are not injected into the prompt.
5. **Hooks** fire during the run when `ai.hooks-enabled = true` (see above).
6. The agent can introspect everything installed at runtime through the read-only
   `list_agent_assets` tool.

Assets are only loaded when `ai.assets-enabled = true` (the default) in
`settings.toml`. The legacy key `skills-enabled` is still accepted as an alias.

## Importing from Cursor

Point **Import folder** at `~/.cursor/skills` (or `~/.cursor/rules`) to migrate a
pack. Folder-style skills are recognised: `…/skills/<name>/SKILL.md` imports as
`<name>.md` and keeps the folder name as the asset name.

## Safety notes

- `.env`, `*.pem`, `*.key`, `id_rsa`, `id_ed25519`, `credentials.json` and
  `secrets.json` are refused by the importer.
- Hooks and MCP servers execute local commands. Review them before enabling, and
  keep `ai.mcp-enabled` and `ai.hooks-enabled` off unless you trust every
  configured entry — both default to `false`.
- A hook is killed after 60 seconds and its failure is reported but never fatal.
- Deleting an asset removes the file from disk; commit your assets to version
  control if you want them recoverable.

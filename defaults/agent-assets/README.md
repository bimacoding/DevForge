# Example agent assets

Copy-paste-ready examples for the six asset families the AI agent understands:
**Skills, MCPs, Subagents, Rules, Commands, Hooks**.

You do not need to copy these by hand. Open **Agent Assets** inside DevForge
(`Settings ▸ Open Agent Assets`, or the ✦ button in the AI composer) and use
**Import file** / **Import folder** to install them.

## Layout

```
defaults/agent-assets/
├── skills/rust-clippy-clean/SKILL.md   # Skills  -> .devforge/skills/
├── mcp/filesystem.json                 # MCPs    -> .devforge/mcp/
├── agents/code-reviewer.md             # Subagents -> .devforge/agents/
├── rules/rust-style.md                 # Rules   -> .devforge/rules/
├── commands/run-tests.md               # Commands -> .devforge/commands/
└── hooks/hooks.json                    # Hooks   -> .devforge/hooks.json
```

## Where assets live

Assets are discovered from four roots, in priority order (project wins):

| Scope   | Roots                                    |
| ------- | ---------------------------------------- |
| Project | `<workspace>/.devforge`, `<workspace>/.cursor` |
| User    | `~/.devforge`, `~/.cursor`               |

Full reference: [`docs/agent-assets.md`](../../docs/agent-assets.md).

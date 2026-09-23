---
name: rust-style
description: Always applied Rust style rules for this workspace
enabled: true
---

- Keep modules small and focused; one public concept per file.
- Public items need a doc comment explaining *why*, not *what*.
- Return `anyhow::Result` from fallible helpers and add context with
  `.with_context(|| format!("... {}", path.display()))`.
- Never `unwrap()` on I/O, parsing, or user input — surface the error instead.
- Comments and identifiers in English; keep user-facing strings consistent with
  the surrounding UI language.
- Any new dependency must be justified in the commit message.

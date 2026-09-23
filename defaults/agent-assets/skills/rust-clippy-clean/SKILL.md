---
name: rust-clippy-clean
description: >-
  Use when writing or reviewing Rust code, especially before claiming a task is
  finished. Keeps the workspace warning-free and idiomatic.
---

# Rust Clippy Clean

Apply this checklist whenever you touch Rust code:

1. Run `cargo check -p <crate>` after every edit and fix errors before moving on.
2. Run `cargo clippy -p <crate> --all-targets` and resolve **new** warnings.
   Do not silence a lint with `#[allow(...)]` unless you explain why.
3. Run `cargo fmt` at the end so the diff stays reviewable.
4. Prefer `&str`/`&Path` parameters over `&String`/`&PathBuf`.
5. Avoid `.clone()` inside hot loops; borrow when ownership is not required.

Report the exact commands you ran and their result.

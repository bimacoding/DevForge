---
name: run-tests
description: Run the workspace test suite and summarise failures
---

Run the following workflow and report the results:

1. `cargo fmt --check` — fail fast if formatting drifted.
2. `cargo test -p devforge-agent -p devforge-app --lib` — the fast unit suite.
3. If step 2 is green, `cargo test --workspace` for the full suite.
4. For every failing test, print the test name, the assertion, and the smallest
   reproducer you can construct.

End with a table: crate | passed | failed | ignored.
Do not attempt to fix anything until the summary is complete.

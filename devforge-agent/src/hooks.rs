//! Lifecycle hooks: run shell commands in response to agent events.
//!
//! A hook bundle is a single `hooks.json` at the root of a config root
//! (`<workspace>/.devforge/hooks.json`, `~/.devforge/hooks.json`, or the
//! `.cursor` equivalents):
//!
//! ```json
//! {
//!   "version": 1,
//!   "hooks": {
//!     "afterFileEdit": [
//!       { "command": "cargo", "args": ["fmt", "--", "--check"], "description": "…" }
//!     ],
//!     "afterAgentRun": [
//!       { "command": "cargo", "args": ["test", "--lib"] }
//!     ]
//!   }
//! }
//! ```
//!
//! Hooks execute **local shell commands**, so running them is opt-in:
//! [`HookRunner::new`] takes an `enabled` flag that callers wire to a user
//! setting. When disabled the runner still parses bundles so the UI can list
//! them, it just refuses to spawn anything.

use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::assets::{AssetKind, AssetScope, discover_kind};

/// Events a hook bundle can subscribe to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HookEvent {
    /// Fired after the agent successfully writes a file.
    AfterFileEdit,
    /// Fired once a run finishes (success, error or cancellation).
    AfterAgentRun,
}

impl HookEvent {
    /// Key used inside the `hooks` object of `hooks.json`.
    pub fn id(self) -> &'static str {
        match self {
            HookEvent::AfterFileEdit => "afterFileEdit",
            HookEvent::AfterAgentRun => "afterAgentRun",
        }
    }

    /// Human-readable label for status messages.
    pub fn label(self) -> &'static str {
        match self {
            HookEvent::AfterFileEdit => "afterFileEdit",
            HookEvent::AfterAgentRun => "afterAgentRun",
        }
    }

    /// Every event, in a stable order.
    pub const ALL: [HookEvent; 2] =
        [HookEvent::AfterFileEdit, HookEvent::AfterAgentRun];
}

/// A single hook command declared in a bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookCommand {
    pub command: String,
    pub args: Vec<String>,
    /// Optional explanation surfaced in the UI.
    pub description: String,
}

impl HookCommand {
    /// Render the command for display.
    pub fn display(&self) -> String {
        if self.args.is_empty() {
            return self.command.clone();
        }
        format!("{} {}", self.command, self.args.join(" "))
    }
}

/// Hooks grouped by event, for one bundle file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookBundle {
    pub by_event: HashMap<HookEvent, Vec<HookCommand>>,
}

impl HookBundle {
    /// Commands registered for one event.
    pub fn for_event(&self, event: HookEvent) -> &[HookCommand] {
        self.by_event.get(&event).map(Vec::as_slice).unwrap_or(&[])
    }

    /// True when the bundle declares no commands at all.
    pub fn is_empty(&self) -> bool {
        self.by_event.values().all(Vec::is_empty)
    }

    /// Total number of commands across every event.
    pub fn len(&self) -> usize {
        self.by_event.values().map(Vec::len).sum()
    }
}

/// Outcome of one executed hook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookOutcome {
    pub event: HookEvent,
    pub command: String,
    pub success: bool,
    /// Combined stdout/stderr, truncated for display.
    pub output: String,
    pub duration_ms: u128,
}

/// Where a hook bundle was loaded from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookSource {
    pub scope: AssetScope,
    pub path: PathBuf,
    pub bundle: HookBundle,
}

/// Parse a `hooks.json` document.
///
/// Unknown event keys are ignored so a bundle written for a newer DevForge
/// keeps working. Commands with an empty `command` are dropped.
pub fn parse_hook_bundle(text: &str) -> Result<HookBundle> {
    let parsed: Value = serde_json::from_str(text).context("parse hooks.json")?;
    let mut bundle = HookBundle::default();
    let Some(hooks) = parsed.get("hooks").and_then(|h| h.as_object()) else {
        return Ok(bundle);
    };
    for (key, value) in hooks {
        let Some(event) = HookEvent::ALL
            .into_iter()
            .find(|e| e.id().eq_ignore_ascii_case(key))
        else {
            continue;
        };
        let Some(items) = value.as_array() else {
            continue;
        };
        let mut commands = Vec::new();
        for item in items {
            let Some(command) = item
                .get("command")
                .and_then(|c| c.as_str())
                .map(str::trim)
                .filter(|c| !c.is_empty())
            else {
                continue;
            };
            let args = item
                .get("args")
                .and_then(|a| a.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let description = item
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or_default()
                .to_string();
            commands.push(HookCommand {
                command: command.to_string(),
                args,
                description,
            });
        }
        if !commands.is_empty() {
            bundle.by_event.insert(event, commands);
        }
    }
    Ok(bundle)
}

/// Discover every hook bundle visible from `workspace_root`.
///
/// Project bundles come first and shadow user bundles of the same file name.
pub fn discover_bundles(workspace_root: Option<&Path>) -> Vec<HookSource> {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for asset in discover_kind(AssetKind::Hooks, workspace_root) {
        if !asset.enabled || seen.contains(&asset.path) {
            continue;
        }
        let Ok(bundle) = parse_hook_bundle(&asset.body) else {
            continue;
        };
        if bundle.is_empty() {
            continue;
        }
        seen.push(asset.path.clone());
        out.push(HookSource {
            scope: asset.scope,
            path: asset.path,
            bundle,
        });
    }
    out
}

/// Merge every discovered bundle into a single view, keyed by event.
pub fn merged_commands(
    workspace_root: Option<&Path>,
    event: HookEvent,
) -> Vec<HookCommand> {
    discover_bundles(workspace_root)
        .into_iter()
        .flat_map(|source| source.bundle.for_event(event).to_vec())
        .collect()
}

/// Maximum wall-clock time a single hook may run.
pub const HOOK_TIMEOUT: Duration = Duration::from_secs(60);

/// Maximum characters of hook output kept for display.
const MAX_HOOK_OUTPUT_CHARS: usize = 4_000;

/// Runs hooks for a workspace.
///
/// Cloning is cheap; the runner holds no mutable state.
#[derive(Clone, Debug)]
pub struct HookRunner {
    root: PathBuf,
    enabled: bool,
}

impl HookRunner {
    /// Create a runner. `enabled` must come from an explicit user setting —
    /// hooks spawn local processes, so they never run by default.
    pub fn new(root: impl Into<PathBuf>, enabled: bool) -> Self {
        Self {
            root: root.into(),
            enabled,
        }
    }

    /// Whether hooks are allowed to execute.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Workspace root used as the working directory for hook processes.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every command registered for an event, across all bundles.
    pub fn commands(&self, event: HookEvent) -> Vec<HookCommand> {
        merged_commands(Some(&self.root), event)
    }

    /// Run every hook for an event, in declaration order.
    ///
    /// Returns an empty vector when hooks are disabled or nothing is
    /// registered. A failing hook never aborts the remaining hooks — the agent
    /// run must not be broken by a broken hook.
    pub fn run(&self, event: HookEvent) -> Vec<HookOutcome> {
        if !self.enabled {
            return Vec::new();
        }
        let mut outcomes = Vec::new();
        for command in self.commands(event) {
            outcomes.push(self.run_one(event, &command));
        }
        outcomes
    }

    /// Execute a single hook command.
    pub fn run_one(&self, event: HookEvent, hook: &HookCommand) -> HookOutcome {
        let started = Instant::now();
        match spawn_with_timeout(&self.root, hook, HOOK_TIMEOUT) {
            Ok((success, output)) => HookOutcome {
                event,
                command: hook.display(),
                success,
                output: truncate(output),
                duration_ms: started.elapsed().as_millis(),
            },
            Err(e) => HookOutcome {
                event,
                command: hook.display(),
                success: false,
                output: truncate(format!("failed to run hook: {e}")),
                duration_ms: started.elapsed().as_millis(),
            },
        }
    }
}

fn truncate(text: String) -> String {
    if text.chars().count() <= MAX_HOOK_OUTPUT_CHARS {
        return text;
    }
    let head: String = text.chars().take(MAX_HOOK_OUTPUT_CHARS).collect();
    let omitted = text.chars().count() - MAX_HOOK_OUTPUT_CHARS;
    format!("{head}\n… ({omitted} more characters omitted)")
}

/// Spawn a hook, wait at most `timeout`, and return `(success, output)`.
///
/// Output is drained on a helper thread so a chatty hook cannot deadlock on a
/// full pipe while we are waiting for the process to exit.
fn spawn_with_timeout(
    cwd: &Path,
    hook: &HookCommand,
    timeout: Duration,
) -> Result<(bool, String)> {
    let mut child = Command::new(&hook.command)
        .args(&hook.args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn `{}`", hook.display()))?;

    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let reader = thread::spawn(move || {
        let mut buf = String::new();
        if let Some(out) = stdout.as_mut() {
            let _ = out.read_to_string(&mut buf);
        }
        if let Some(err) = stderr.as_mut() {
            let _ = err.read_to_string(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().context("wait for hook")? {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        thread::sleep(Duration::from_millis(25));
    };

    let output = reader.join().unwrap_or_default();
    match status {
        Some(status) => Ok((status.success(), output)),
        None => Ok((
            false,
            format!(
                "{output}\n… hook exceeded {}s and was terminated",
                timeout.as_secs()
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn write_bundle(root: &Path, body: &str) {
        let dir = root.join(".devforge");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("hooks.json"), body).unwrap();
    }

    #[test]
    fn parses_bundle_and_ignores_unknown_events() {
        let bundle = parse_hook_bundle(
            r#"{
                "version": 1,
                "hooks": {
                    "afterFileEdit": [
                        {"command": "cargo", "args": ["fmt"], "description": "format"}
                    ],
                    "onSave": [{"command": "nope"}],
                    "afterAgentRun": [{"command": "cargo", "args": ["test"]}]
                }
            }"#,
        )
        .unwrap();

        let edits = bundle.for_event(HookEvent::AfterFileEdit);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].command, "cargo");
        assert_eq!(edits[0].args, vec!["fmt".to_string()]);
        assert_eq!(edits[0].description, "format");
        assert_eq!(bundle.for_event(HookEvent::AfterAgentRun).len(), 1);
        assert_eq!(bundle.len(), 2);
    }

    #[test]
    fn drops_commands_without_a_command_field() {
        let bundle = parse_hook_bundle(
            r#"{"hooks":{"afterFileEdit":[{"args":["x"]},{"command":"   "}]}}"#,
        )
        .unwrap();
        assert!(bundle.is_empty());
        assert!(bundle.for_event(HookEvent::AfterFileEdit).is_empty());
    }

    #[test]
    fn malformed_json_is_an_error() {
        assert!(parse_hook_bundle("{not json").is_err());
        assert!(parse_hook_bundle(r#"{"version":1}"#).unwrap().is_empty());
    }

    #[test]
    fn discovers_project_bundles() {
        let root = temp_root();
        write_bundle(
            root.path(),
            r#"{"hooks":{"afterFileEdit":[{"command":"cargo","args":["fmt"]}]}}"#,
        );
        let sources = discover_bundles(Some(root.path()));
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].scope, AssetScope::Project);
        assert!(sources[0].path.ends_with("hooks.json"));

        let merged = merged_commands(Some(root.path()), HookEvent::AfterFileEdit);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].display(), "cargo fmt");
    }

    #[test]
    fn disabled_runner_executes_nothing() {
        let root = temp_root();
        write_bundle(
            root.path(),
            r#"{"hooks":{"afterAgentRun":[{"command":"touch","args":["ran.txt"]}]}}"#,
        );
        let runner = HookRunner::new(root.path(), false);
        assert!(!runner.is_enabled());
        // The bundle is still visible so the UI can list it…
        assert_eq!(runner.commands(HookEvent::AfterAgentRun).len(), 1);
        // …but nothing runs.
        assert!(runner.run(HookEvent::AfterAgentRun).is_empty());
        assert!(!root.path().join("ran.txt").exists());
    }

    #[test]
    fn enabled_runner_executes_and_reports_success() {
        let root = temp_root();
        write_bundle(
            root.path(),
            r#"{"hooks":{"afterAgentRun":[{"command":"touch","args":["ran.txt"]}]}}"#,
        );
        let runner = HookRunner::new(root.path(), true);
        let outcomes = runner.run(HookEvent::AfterAgentRun);
        assert_eq!(outcomes.len(), 1);
        assert!(outcomes[0].success, "output: {}", outcomes[0].output);
        assert!(root.path().join("ran.txt").exists());
    }

    #[test]
    fn failing_hook_is_reported_without_panicking() {
        let root = temp_root();
        write_bundle(
            root.path(),
            r#"{"hooks":{"afterAgentRun":[{"command":"sh","args":["-c","echo boom >&2; exit 3"]}]}}"#,
        );
        let runner = HookRunner::new(root.path(), true);
        let outcomes = runner.run(HookEvent::AfterAgentRun);
        assert_eq!(outcomes.len(), 1);
        assert!(!outcomes[0].success);
        assert!(outcomes[0].output.contains("boom"));
    }

    #[test]
    fn missing_binary_is_reported_as_failure() {
        let root = temp_root();
        let runner = HookRunner::new(root.path(), true);
        let outcome = runner.run_one(
            HookEvent::AfterAgentRun,
            &HookCommand {
                command: "definitely-not-a-real-binary-xyz".into(),
                args: Vec::new(),
                description: String::new(),
            },
        );
        assert!(!outcome.success);
        assert!(outcome.output.contains("failed to run hook"));
    }

    #[test]
    fn runs_every_hook_even_when_one_fails() {
        let root = temp_root();
        write_bundle(
            root.path(),
            r#"{"hooks":{"afterAgentRun":[
                {"command":"sh","args":["-c","exit 1"]},
                {"command":"touch","args":["second.txt"]}
            ]}}"#,
        );
        let runner = HookRunner::new(root.path(), true);
        let outcomes = runner.run(HookEvent::AfterAgentRun);
        assert_eq!(outcomes.len(), 2);
        assert!(!outcomes[0].success);
        assert!(outcomes[1].success);
        assert!(root.path().join("second.txt").exists());
    }

    #[test]
    fn hooks_run_inside_the_workspace_root() {
        let root = temp_root();
        write_bundle(
            root.path(),
            r#"{"hooks":{"afterAgentRun":[{"command":"sh","args":["-c","pwd > where.txt"]}]}}"#,
        );
        let runner = HookRunner::new(root.path(), true);
        runner.run(HookEvent::AfterAgentRun);
        let cwd = fs::read_to_string(root.path().join("where.txt")).unwrap();
        // macOS resolves /var -> /private/var, so compare the final component.
        assert!(
            cwd.trim().ends_with(
                root.path().file_name().unwrap().to_string_lossy().as_ref()
            ),
            "cwd was {cwd}"
        );
    }

    /// The bundle shipped in `defaults/agent-assets/` must stay parseable, so
    /// the documented example cannot silently drift from the parser.
    #[test]
    fn shipped_example_bundle_is_valid() {
        let bundle = parse_hook_bundle(include_str!(
            "../../defaults/agent-assets/hooks/hooks.json"
        ))
        .unwrap();
        assert!(!bundle.is_empty());
        assert_eq!(bundle.for_event(HookEvent::AfterFileEdit).len(), 1);
        assert_eq!(bundle.for_event(HookEvent::AfterAgentRun).len(), 1);
        for event in HookEvent::ALL {
            for command in bundle.for_event(event) {
                assert!(
                    !command.command.trim().is_empty(),
                    "{} hook has an empty command",
                    event.label()
                );
            }
        }
    }

    #[test]
    fn event_ids_match_the_documented_keys() {
        assert_eq!(HookEvent::AfterFileEdit.id(), "afterFileEdit");
        assert_eq!(HookEvent::AfterAgentRun.id(), "afterAgentRun");
    }
}

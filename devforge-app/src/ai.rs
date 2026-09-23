use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use base64::Engine;
use devforge_agent::{AgentMode, can_start_another_run, clamp_max_parallel};
use devforge_core::{command::EditCommand, directory::Directory, mode::Mode};
use floem::{
    action::{open_file, show_context_menu},
    ext_event::create_signal_from_channel,
    file::FileDialogOptions,
    keyboard::Modifiers,
    menu::{Menu, MenuItem},
    reactive::{
        RwSignal, Scope, SignalGet, SignalUpdate, SignalWith, create_effect,
    },
};
use im::Vector;
use lapce_xi_rope::Rope;
use serde::{Deserialize, Serialize};

use crate::{
    ai_providers::{self, model_picker_list, provider_preset},
    command::{CommandExecuted, CommandKind, LapceWorkbenchCommand},
    editor::EditorData,
    keypress::{KeyPressFocus, condition::Condition},
    main_split::Editors,
    window_tab::CommonData,
    workspace::{LapceWorkspace, LapceWorkspaceType},
};

const HISTORY_FILE: &str = "ai_conversations.json";
const MAX_TEXT_ATTACH_BYTES: u64 = 512_000;
const MAX_IMAGE_ATTACH_BYTES: u64 = 4_000_000;

#[derive(Clone, Debug)]
pub struct QueuedPrompt {
    pub id: String,
    pub prompt: String,
    pub attachments: Vec<AiAttachment>,
}

#[derive(Clone)]
pub struct PendingToolApproval {
    pub name: String,
    pub arguments: String,
    pub reply: std::sync::mpsc::Sender<bool>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AiAttachmentKind {
    Image,
    Text,
    Document,
    Directory,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiAttachment {
    pub path: PathBuf,
    pub kind: AiAttachmentKind,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AiChatRole {
    User,
    Assistant,
    System,
    Activity,
}

impl AiChatRole {
    pub fn key(&self) -> u8 {
        match self {
            Self::User => 0,
            Self::Assistant => 1,
            Self::System => 2,
            Self::Activity => 3,
        }
    }
}

/// How an agent step (tool / MCP call) finished, used for the step icon + tint.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AiStepState {
    Running,
    Done,
    Failed,
    Skipped,
}

/// One observable agent step: a tool/MCP call plus what it returned.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiAgentStep {
    pub name: String,
    #[serde(default)]
    pub arguments: String,
    #[serde(default)]
    pub output: String,
    pub state: AiStepState,
    #[serde(default)]
    pub is_mcp: bool,
    /// Files touched by this step (for write tools), rendered as chips.
    #[serde(default)]
    pub files: Vec<String>,
}

impl AiAgentStep {
    pub fn new(name: String, arguments: String, is_mcp: bool) -> Self {
        Self {
            files: touched_files(&name, &arguments),
            name,
            arguments,
            output: String::new(),
            state: AiStepState::Running,
            is_mcp,
        }
    }
}

/// Compact one-line summary of a step, e.g. ``str_replace · src/main.rs``.
pub fn step_summary(step: &AiAgentStep) -> String {
    let verb = tool_verb(&step.name);
    match step.files.first() {
        Some(file) => format!("{verb} · {file}"),
        None => verb.to_string(),
    }
}

pub fn tool_verb(name: &str) -> &'static str {
    if let Some(rest) = name.strip_prefix("mcp__") {
        return match rest.split("__").nth(1) {
            Some(sub) => match sub {
                "list_tables" => "List tables",
                "describe_table" => "Inspect table",
                "execute_query" => "Run query",
                _ => "MCP call",
            },
            None => "MCP call",
        };
    }
    match name {
        "read_file" => "Read file",
        "list_directory" => "List directory",
        "get_project_structure" => "Map project",
        "search_code" => "Search code",
        "write_file" => "Write file",
        "str_replace" => "Edit file",
        "create_directory" => "Create directory",
        _ => "Tool call",
    }
}

/// Best-effort extraction of `path`-like arguments for file chips.
fn touched_files(name: &str, arguments: &str) -> Vec<String> {
    if !matches!(
        name,
        "read_file" | "write_file" | "str_replace" | "create_directory"
    ) {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return Vec::new();
    };
    value
        .get("path")
        .and_then(|p| p.as_str())
        .map(|p| vec![p.to_string()])
        .unwrap_or_default()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiChatMessage {
    pub role: AiChatRole,
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<String>,
    /// Populated for `AiChatRole::Activity` messages produced by tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<AiAgentStep>,
}

impl AiChatMessage {
    pub fn new(role: AiChatRole, content: String) -> Self {
        Self {
            role,
            content,
            attachments: Vec::new(),
            step: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiConversation {
    pub id: String,
    pub title: String,
    pub mode: String,
    pub model: String,
    pub messages: Vec<AiChatMessage>,
    pub updated_at: u64,
}

impl AiConversation {
    pub fn new(mode: AgentMode, model: String) -> Self {
        Self {
            id: format!("chat-{}", now_secs_nanos()),
            title: "New chat".into(),
            mode: mode.as_str().into(),
            model,
            messages: Vec::new(),
            updated_at: now_secs(),
        }
    }
}

#[derive(Clone)]
pub struct AiData {
    pub common: Rc<CommonData>,
    pub query_editor: EditorData,
    /// Active conversation messages (synced from conversations list).
    pub messages: RwSignal<Vector<AiChatMessage>>,
    pub conversations: RwSignal<Vector<AiConversation>>,
    pub active_id: RwSignal<String>,
    pub mode: RwSignal<AgentMode>,
    /// Empty / "Auto" → use settings model.
    pub model: RwSignal<String>,
    pub attachments: RwSignal<Vector<AiAttachment>>,
    pub status: RwSignal<String>,
    pub busy: RwSignal<bool>,
    pub listening: RwSignal<bool>,
    pub cancel: RwSignal<Arc<AtomicBool>>,
    /// Cancel tokens for in-flight runs, keyed by conversation id.
    pub active_runs: RwSignal<HashMap<String, Arc<AtomicBool>>>,
    /// Prompts waiting while a run is in progress.
    pub queue: RwSignal<Vector<QueuedPrompt>>,
    /// Tool / MCP call awaiting Run or Skip.
    pub pending_tool: RwSignal<Option<PendingToolApproval>>,
}

impl std::fmt::Debug for AiData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiData")
            .field("status", &self.status.get_untracked())
            .field("busy", &self.busy.get_untracked())
            .finish()
    }
}

impl KeyPressFocus for AiData {
    fn get_mode(&self) -> Mode {
        Mode::Insert
    }

    fn check_condition(&self, condition: Condition) -> bool {
        matches!(condition, Condition::PanelFocus | Condition::AiFocus)
    }

    fn run_command(
        &self,
        command: &crate::command::LapceCommand,
        count: Option<usize>,
        mods: Modifiers,
    ) -> CommandExecuted {
        match &command.kind {
            CommandKind::Edit(edit) if *edit == EditCommand::InsertNewLine => {
                // Enter = send; Shift+Enter = newline (Cursor-style)
                if mods.shift() {
                    return self.query_editor.run_command(command, count, mods);
                }
                self.send(self.common.workspace.clone());
                CommandExecuted::Yes
            }
            CommandKind::Edit(_)
            | CommandKind::Move(_)
            | CommandKind::MultiSelection(_) => {
                self.query_editor.run_command(command, count, mods)
            }
            CommandKind::Workbench(_)
            | CommandKind::Scroll(_)
            | CommandKind::Focus(_)
            | CommandKind::MotionMode(_) => CommandExecuted::No,
        }
    }

    fn receive_char(&self, c: &str) {
        self.query_editor.receive_char(c);
    }
}

#[derive(Clone)]
enum AiUiEvent {
    Status {
        conv_id: String,
        text: String,
    },
    Append {
        conv_id: String,
        msg: AiChatMessage,
    },
    /// Start an empty assistant bubble for SSE streaming.
    BeginAssistant {
        conv_id: String,
    },
    /// Append text to the last assistant message (token / chunk stream).
    AssistantDelta {
        conv_id: String,
        delta: String,
    },
    SetBusy {
        conv_id: String,
        busy: bool,
    },
    /// A tool / MCP call just started; push a collapsible step bubble.
    StepStart {
        conv_id: String,
        name: String,
        arguments: String,
        is_mcp: bool,
    },
    /// A tool / MCP call finished; fill in output + state of the open step.
    StepEnd {
        conv_id: String,
        name: String,
        output: String,
        is_error: bool,
    },
    NeedApproval {
        conv_id: String,
        name: String,
        arguments: String,
        reply: mpsc::Sender<bool>,
    },
}

impl AiData {
    pub fn new(cx: Scope, editors: Editors, common: Rc<CommonData>) -> Self {
        let query_editor = editors.make_local(cx, common.clone());
        let config = common.config.get_untracked();
        let default_mode = AgentMode::from_str_loose(&config.ai.default_mode);
        let stored = load_conversations();
        let (conversations, active_id, messages, mode, model) =
            if let Some((list, active)) = stored {
                let active_conv = list
                    .iter()
                    .find(|c| c.id == active)
                    .cloned()
                    .or_else(|| list.first().cloned());
                if let Some(conv) = active_conv {
                    let msgs: Vector<_> = conv.messages.iter().cloned().collect();
                    let mode = AgentMode::from_str_loose(&conv.mode);
                    let model = if conv.model.is_empty() {
                        "Auto".into()
                    } else {
                        conv.model.clone()
                    };
                    (Vector::from(list), conv.id, msgs, mode, model)
                } else {
                    let conv = AiConversation::new(default_mode, "Auto".into());
                    (
                        Vector::unit(conv.clone()),
                        conv.id,
                        Vector::new(),
                        default_mode,
                        "Auto".into(),
                    )
                }
            } else {
                let conv = AiConversation::new(default_mode, "Auto".into());
                (
                    Vector::unit(conv.clone()),
                    conv.id,
                    Vector::new(),
                    default_mode,
                    "Auto".into(),
                )
            };

        Self {
            common,
            query_editor,
            messages: cx.create_rw_signal(messages),
            conversations: cx.create_rw_signal(conversations),
            active_id: cx.create_rw_signal(active_id),
            mode: cx.create_rw_signal(mode),
            model: cx.create_rw_signal(model),
            attachments: cx.create_rw_signal(Vector::new()),
            status: cx.create_rw_signal("Idle".into()),
            busy: cx.create_rw_signal(false),
            listening: cx.create_rw_signal(false),
            cancel: cx.create_rw_signal(Arc::new(AtomicBool::new(false))),
            active_runs: cx.create_rw_signal(HashMap::new()),
            queue: cx.create_rw_signal(Vector::new()),
            pending_tool: cx.create_rw_signal(None),
        }
    }

    pub fn resolved_model(&self) -> String {
        let selected = self.model.get_untracked();
        if selected.is_empty() || selected.eq_ignore_ascii_case("auto") {
            self.common.config.get_untracked().ai.model.clone()
        } else {
            selected
        }
    }

    pub fn new_conversation(&self) {
        self.persist_active();
        let mode = self.mode.get_untracked();
        let model = self.model.get_untracked();
        let conv = AiConversation::new(mode, model);
        let id = conv.id.clone();
        self.conversations.update(|list| {
            list.push_front(conv);
        });
        self.active_id.set(id);
        self.messages.set(Vector::new());
        self.attachments.set(Vector::new());
        self.status.set("Idle".into());
        self.persist_all();
    }

    pub fn select_conversation(&self, id: &str) {
        if id == self.active_id.get_untracked() {
            return;
        }
        self.persist_active();
        let Some(conv) = self
            .conversations
            .with_untracked(|list| list.iter().find(|c| c.id == id).cloned())
        else {
            return;
        };
        self.active_id.set(conv.id.clone());
        self.mode.set(AgentMode::from_str_loose(&conv.mode));
        self.model.set(if conv.model.is_empty() {
            "Auto".into()
        } else {
            conv.model
        });
        self.messages.set(conv.messages.into_iter().collect());
        self.attachments.set(Vector::new());
        self.persist_all();
    }

    pub fn delete_conversation(&self, id: &str) {
        let was_active = self.active_id.get_untracked() == id;
        self.conversations.update(|list| {
            if let Some(idx) = list.iter().position(|c| c.id == id) {
                list.remove(idx);
            }
        });
        if self.conversations.with_untracked(|l| l.is_empty()) {
            let mode = self.mode.get_untracked();
            let model = self.model.get_untracked();
            let conv = AiConversation::new(mode, model);
            self.active_id.set(conv.id.clone());
            self.messages.set(Vector::new());
            self.conversations.set(Vector::unit(conv));
        } else if was_active {
            let next = self
                .conversations
                .with_untracked(|l| l.front().map(|c| c.id.clone()));
            if let Some(next) = next {
                self.select_conversation(&next);
            }
        }
        self.persist_all();
    }

    fn persist_active(&self) {
        let id = self.active_id.get_untracked();
        let msgs: Vec<_> = self.messages.get_untracked().into_iter().collect();
        let mode = self.mode.get_untracked().as_str().to_string();
        let model = self.model.get_untracked();
        let title = derive_title(&msgs);
        self.conversations.update(|list| {
            if let Some(conv) = list.iter_mut().find(|c| c.id == id) {
                conv.messages = msgs;
                conv.mode = mode;
                conv.model = model;
                conv.title = title;
                conv.updated_at = now_secs();
            }
        });
    }

    fn persist_all(&self) {
        self.persist_active();
        let list: Vec<_> = self.conversations.get_untracked().into_iter().collect();
        let active = self.active_id.get_untracked();
        save_conversations(&list, &active);
    }

    pub fn set_mode(&self, mode: AgentMode) {
        self.mode.set(mode);
        self.persist_active();
        self.persist_all();
    }

    pub fn set_model(&self, model: String) {
        self.model.set(model);
        self.persist_active();
        self.persist_all();
    }

    pub fn show_mode_menu(&self) {
        let ai = self.clone();
        let mut menu = Menu::new("");
        for mode in [AgentMode::Ask, AgentMode::Agent, AgentMode::Edit] {
            let label = match mode {
                AgentMode::Ask => "Ask — read-only Q&A",
                AgentMode::Agent => "Agent — inspect + write tools",
                AgentMode::Edit => "Edit — focused file edits",
            };
            let ai = ai.clone();
            menu = menu.entry(MenuItem::new(label).action(move || {
                ai.set_mode(mode);
                ai.status.set(format!("{} mode", mode.label()));
            }));
        }
        show_context_menu(menu, None);
    }

    pub fn show_model_menu(&self) {
        let ai = self.clone();
        let config = ai.common.config.get_untracked();
        let current = ai.resolved_model();
        let models = model_picker_list(&config.ai.provider, &config.ai.extra_models);
        let mut menu = Menu::new("");
        for name in models {
            let ai = ai.clone();
            let mark = if name.eq_ignore_ascii_case("auto") {
                format!("Auto ({current})")
            } else if name == ai.model.get_untracked() {
                format!("✓ {name}")
            } else {
                name.clone()
            };
            let name_set = name.clone();
            menu = menu.entry(MenuItem::new(mark).action(move || {
                ai.set_model(name_set.clone());
            }));
        }
        menu = menu.separator();
        let ai_add = self.clone();
        menu = menu.entry(MenuItem::new("Add model from input…").action(move || {
            let typed = ai_add
                .query_editor
                .doc()
                .buffer
                .with_untracked(|b| b.to_string())
                .trim()
                .to_string();
            if typed.is_empty() || typed.contains(' ') {
                ai_add.status.set(
                    "Type a model id in the composer (e.g. openai/gpt-4o), then Add model"
                        .into(),
                );
                return;
            }
            ai_add.add_extra_model(&typed);
            ai_add.set_model(typed);
            ai_add
                .query_editor
                .doc()
                .reload(Rope::from(""), true);
        }));
        show_context_menu(menu, None);
    }

    pub fn show_provider_menu(&self) {
        let ai = self.clone();
        let current = ai.common.config.get_untracked().ai.provider.clone();
        let mut menu = Menu::new("");
        for preset in ai_providers::PROVIDER_PRESETS {
            let ai = ai.clone();
            let id = preset.id.to_string();
            let mark = if id.eq_ignore_ascii_case(&current) {
                format!("✓ {} ({})", preset.label, preset.id)
            } else {
                format!("{} ({})", preset.label, preset.id)
            };
            menu = menu.entry(MenuItem::new(mark).action(move || {
                ai.apply_provider(&id);
            }));
        }
        show_context_menu(menu, None);
    }

    pub fn show_history_menu(&self) {
        let ai = self.clone();
        let active = ai.active_id.get_untracked();
        let list: Vec<(String, String)> = ai.conversations.with_untracked(|c| {
            c.iter().map(|c| (c.id.clone(), c.title.clone())).collect()
        });
        let mut menu = Menu::new("");
        for (id, title) in list {
            let ai = ai.clone();
            let mark = if id == active {
                format!("✓ {title}")
            } else {
                title.clone()
            };
            let id_sel = id.clone();
            menu = menu.entry(MenuItem::new(mark).action(move || {
                ai.select_conversation(&id_sel);
            }));
        }
        menu = menu.separator();
        let ai_clear = self.clone();
        menu = menu.entry(MenuItem::new("Clear current chat").action(move || {
            ai_clear.clear_active_chat();
        }));
        let ai_del = self.clone();
        let active_del = active.clone();
        menu = menu.entry(MenuItem::new("Delete current chat").action(move || {
            ai_del.delete_conversation(&active_del);
        }));
        show_context_menu(menu, None);
    }

    /// Title of the active conversation, or a placeholder when it has none yet.
    pub fn active_title(&self) -> String {
        let id = self.active_id.get_untracked();
        let list: Vec<AiConversation> =
            self.conversations.get_untracked().into_iter().collect();
        active_title_from(&list, &id)
    }

    /// Files touched by Agent steps in the active conversation, in order.
    pub fn touched_files(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        self.messages.with_untracked(|msgs| {
            for msg in msgs.iter() {
                let Some(step) = msg.step.as_ref() else {
                    continue;
                };
                for file in &step.files {
                    if !out.contains(file) {
                        out.push(file.clone());
                    }
                }
            }
        });
        out
    }

    pub fn clear_active_chat(&self) {
        self.messages.set(Vector::new());
        self.persist_all();
    }

    /// Open the Agent Assets page (Skills, MCPs, Subagents, Rules, …).
    pub fn open_agent_assets(&self) {
        self.common
            .workbench_command
            .send(LapceWorkbenchCommand::OpenAgentAssets);
        self.status.set("Opened Agent Assets".into());
    }

    /// Re-send the last user prompt in this conversation.
    pub fn retry_last(&self, workspace: Arc<LapceWorkspace>) {
        if self.busy.get_untracked() {
            self.status.set("Busy — stop the run first".into());
            return;
        }
        let last = self.messages.with_untracked(|msgs| {
            msgs.iter()
                .rev()
                .find(|m| matches!(m.role, AiChatRole::User))
                .map(|m| m.content.clone())
        });
        let Some(mut prompt) = last else {
            self.status.set("Nothing to retry yet".into());
            return;
        };
        // Drop the citation block appended by dispatch_prompt.
        if let Some(idx) = prompt.find("\n\n@") {
            prompt.truncate(idx);
        }
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            self.status.set("Nothing to retry yet".into());
            return;
        }
        self.dispatch_prompt(workspace, prompt, Vec::new());
    }

    fn apply_provider(&self, provider: &str) {
        use crate::config::LapceConfig;
        if let Ok(value) = serde::Serialize::serialize(
            &provider,
            toml_edit::ser::ValueSerializer::new(),
        ) {
            LapceConfig::update_file("ai", "provider", value);
        }
        if let Some(preset) = provider_preset(provider) {
            if let Ok(value) = serde::Serialize::serialize(
                &preset.default_base_url,
                toml_edit::ser::ValueSerializer::new(),
            ) {
                LapceConfig::update_file("ai", "base-url", value);
            }
            if let Some(first) = preset.models.first() {
                if let Ok(value) = serde::Serialize::serialize(
                    first,
                    toml_edit::ser::ValueSerializer::new(),
                ) {
                    LapceConfig::update_file("ai", "model", value);
                }
                self.set_model((*first).to_string());
            }
            self.status.set(format!(
                "Provider → {} · set API key via env {:?}",
                preset.label, preset.env_keys
            ));
        }
    }

    pub fn add_extra_model(&self, model: &str) {
        use crate::config::LapceConfig;
        let config = self.common.config.get_untracked();
        let mut list: Vec<String> = config
            .ai
            .extra_models
            .split([',', '\n', ';'])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if list.iter().any(|m| m.eq_ignore_ascii_case(model)) {
            return;
        }
        list.push(model.to_string());
        let joined = list.join(", ");
        if let Ok(value) = serde::Serialize::serialize(
            &joined,
            toml_edit::ser::ValueSerializer::new(),
        ) {
            LapceConfig::update_file("ai", "extra-models", value);
        }
        self.status.set(format!("Added model `{model}` to picker"));
    }

    pub fn pick_attachments(&self) {
        let ai = self.clone();
        let options = FileDialogOptions::new()
            .title("Attach files for AI")
            .multi_selection();
        open_file(options, move |files| {
            let Some(info) = files else {
                return;
            };
            for path in info.path {
                ai.add_attachment(path);
            }
        });
    }

    pub fn add_attachment(&self, path: PathBuf) {
        let kind = classify_attachment(&path);
        let name = citation_label(&path, self.common.workspace.path.as_deref());
        let exists = self
            .attachments
            .with_untracked(|a| a.iter().any(|x| x.path == path));
        if exists {
            self.status.set(format!("Already attached `{name}`"));
            return;
        }
        self.attachments.update(|a| {
            a.push_back(AiAttachment {
                path,
                kind,
                name: name.clone(),
            });
        });
        self.status.set(format!("Attached `{name}` to chat"));
    }

    pub fn remove_attachment(&self, path: &Path) {
        self.attachments.update(|a| {
            if let Some(i) = a.iter().position(|x| x.path == path) {
                a.remove(i);
            }
        });
    }

    pub fn toggle_speech(&self) {
        if self.listening.get_untracked() {
            self.listening.set(false);
            self.status.set("Speech input stopped".into());
            return;
        }
        self.listening.set(true);
        #[cfg(target_os = "macos")]
        {
            self.status.set(
                "Listening… gunakan Dictation macOS (Fn Fn / Globe) di kotak input, lalu klik Mic lagi"
                    .into(),
            );
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.status.set(
                "Listening… gunakan system speech-to-text di kotak input, lalu klik Mic lagi"
                    .into(),
            );
        }
    }

    pub fn stop(&self) {
        self.active_runs.with_untracked(|runs| {
            for cancel in runs.values() {
                cancel.store(true, Ordering::SeqCst);
            }
        });
        self.cancel.get_untracked().store(true, Ordering::SeqCst);
        if let Some(pending) = self.pending_tool.get_untracked() {
            let _ = pending.reply.send(false);
            self.pending_tool.set(None);
        }
        self.status.set("Cancelled".into());
        self.active_runs.set(HashMap::new());
        self.busy.set(false);
        self.listening.set(false);
        self.drain_queue();
    }

    pub fn approve_pending_tool(&self, run: bool) {
        if let Some(pending) = self.pending_tool.get_untracked() {
            let _ = pending.reply.send(run);
            self.pending_tool.set(None);
            self.status.set(if run {
                "Tool approved — running…".into()
            } else {
                "Tool skipped".into()
            });
        }
    }

    pub fn skip_queued(&self, id: &str) {
        self.queue.update(|q| {
            if let Some(i) = q.iter().position(|p| p.id == id) {
                q.remove(i);
            }
        });
    }

    pub fn clear_queue(&self) {
        self.queue.set(Vector::new());
    }

    /// Cancel the current run and start this queued prompt immediately.
    pub fn run_queued_now(&self, id: &str, workspace: Arc<LapceWorkspace>) {
        let item = self
            .queue
            .with_untracked(|q| q.iter().find(|p| p.id == id).cloned());
        let Some(item) = item else {
            return;
        };
        self.queue.update(|q| {
            if let Some(i) = q.iter().position(|p| p.id == id) {
                q.remove(i);
            }
        });
        // Interrupt current work, then run this prompt next.
        self.cancel.get_untracked().store(true, Ordering::SeqCst);
        let active = self.active_id.get_untracked();
        if let Some(c) = self.active_runs.with_untracked(|m| m.get(&active).cloned())
        {
            c.store(true, Ordering::SeqCst);
        }
        if let Some(pending) = self.pending_tool.get_untracked() {
            let _ = pending.reply.send(false);
            self.pending_tool.set(None);
        }
        self.active_runs.update(|m| {
            m.remove(&active);
        });
        self.busy
            .set(!self.active_runs.with_untracked(|m| m.is_empty()));
        self.status.set("Switching to queued prompt…".into());
        self.dispatch_prompt(workspace, item.prompt, item.attachments);
    }

    fn drain_queue(&self) {
        if self.busy.get_untracked() {
            return;
        }
        let next = self.queue.with_untracked(|q| q.front().cloned());
        let Some(next) = next else {
            return;
        };
        self.queue.update(|q| {
            q.pop_front();
        });
        let workspace = self.common.workspace.clone();
        self.dispatch_prompt(workspace, next.prompt, next.attachments);
    }

    pub fn send(&self, workspace: Arc<LapceWorkspace>) {
        let config = self.common.config.get_untracked();
        if !config.ai.enabled {
            self.status.set("AI is disabled in settings".into());
            return;
        }

        let prompt = self
            .query_editor
            .doc()
            .buffer
            .with_untracked(|b| b.to_string())
            .trim()
            .to_string();
        let attachments: Vec<AiAttachment> =
            self.attachments.get_untracked().into_iter().collect();
        if prompt.is_empty() && attachments.is_empty() {
            return;
        }

        self.query_editor.doc().reload(Rope::from(""), true);
        self.attachments.set(Vector::new());
        self.listening.set(false);

        let conv_id = self.active_id.get_untracked();
        let conv_busy = self
            .active_runs
            .with_untracked(|m| m.contains_key(&conv_id));
        let active_count = self.active_runs.with_untracked(|m| m.len());
        let max_parallel = clamp_max_parallel(config.ai.max_parallel_runs);
        let can_parallel =
            !conv_busy && can_start_another_run(active_count, max_parallel);

        if !can_parallel {
            let id = format!("q-{}", now_secs_nanos());
            self.queue.update(|q| {
                q.push_back(QueuedPrompt {
                    id: id.clone(),
                    prompt,
                    attachments,
                });
            });
            let n = self.queue.with_untracked(|q| q.len());
            let reason = if conv_busy {
                "this chat is busy"
            } else {
                "parallel limit reached"
            };
            self.status
                .set(format!("Queued ({n}, {reason}) — Run Now / Skip, or wait"));
            return;
        }

        self.dispatch_prompt(workspace, prompt, attachments);
    }

    fn dispatch_prompt(
        &self,
        workspace: Arc<LapceWorkspace>,
        prompt: String,
        attachments: Vec<AiAttachment>,
    ) {
        let config = self.common.config.get_untracked();
        let attach_names: Vec<String> =
            attachments.iter().map(|a| a.name.clone()).collect();
        let display = if attachments.is_empty() {
            prompt.clone()
        } else if prompt.is_empty() {
            attach_names
                .iter()
                .map(|n| format!("@{n}"))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            let cites = attach_names
                .iter()
                .map(|n| format!("@{n}"))
                .collect::<Vec<_>>()
                .join(" ");
            format!("{prompt}\n\n{cites}")
        };

        let history: Vec<(AiChatRole, String)> = self
            .messages
            .get_untracked()
            .into_iter()
            .filter(|m| matches!(m.role, AiChatRole::User | AiChatRole::Assistant))
            .map(|m| (m.role.clone(), m.content.clone()))
            .collect();

        let mut msgs = self.messages.get_untracked();
        msgs.push_back(AiChatMessage {
            role: AiChatRole::User,
            content: display,
            attachments: attach_names,
            step: None,
        });
        self.messages.set(msgs);
        self.persist_active();

        let conv_id = self.active_id.get_untracked();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel.set(cancel.clone());
        self.active_runs.update(|m| {
            m.insert(conv_id.clone(), cancel.clone());
        });
        self.busy.set(true);
        self.status.set("Analyzing".into());

        let messages = self.messages;
        let status = self.status;
        let busy = self.busy;
        let conversations = self.conversations;
        let active_id = self.active_id;
        let active_runs = self.active_runs;
        let pending_tool = self.pending_tool;
        let queue = self.queue;
        let ai_self = self.clone();
        let mut ai_cfg = config.ai.clone();
        ai_cfg.model = self.resolved_model();
        let mode = self.mode.get_untracked();
        let run_conv_id = conv_id.clone();

        let (tx, rx) = mpsc::channel::<AiUiEvent>();
        let notification = create_signal_from_channel(rx);
        create_effect(move |_| {
            notification.with(|ev| {
                if let Some(ev) = ev.as_ref() {
                    match ev {
                        AiUiEvent::Status { conv_id, text } => {
                            if active_id.get_untracked() == *conv_id {
                                status.set(text.clone());
                            }
                        }
                        AiUiEvent::Append { conv_id, msg } => {
                            mutate_conv_messages(
                                conversations,
                                messages,
                                active_id,
                                conv_id,
                                |msgs| msgs.push(msg.clone()),
                            );
                            let list: Vec<_> =
                                conversations.get_untracked().into_iter().collect();
                            let aid = active_id.get_untracked();
                            save_conversations(&list, &aid);
                        }
                        AiUiEvent::BeginAssistant { conv_id } => {
                            mutate_conv_messages(
                                conversations,
                                messages,
                                active_id,
                                conv_id,
                                |msgs| {
                                    msgs.push(AiChatMessage {
                                        role: AiChatRole::Assistant,
                                        content: String::new(),
                                        attachments: Vec::new(),
                                        step: None,
                                    });
                                },
                            );
                        }
                        AiUiEvent::AssistantDelta { conv_id, delta } => {
                            mutate_conv_messages(
                                conversations,
                                messages,
                                active_id,
                                conv_id,
                                |msgs| {
                                    if let Some(last) = msgs.last_mut() {
                                        if matches!(last.role, AiChatRole::Assistant)
                                        {
                                            last.content.push_str(delta);
                                            return;
                                        }
                                    }
                                    msgs.push(AiChatMessage {
                                        role: AiChatRole::Assistant,
                                        content: delta.clone(),
                                        attachments: Vec::new(),
                                        step: None,
                                    });
                                },
                            );
                        }
                        AiUiEvent::StepStart {
                            conv_id,
                            name,
                            arguments,
                            is_mcp,
                        } => {
                            mutate_conv_messages(
                                conversations,
                                messages,
                                active_id,
                                conv_id,
                                |msgs| {
                                    msgs.push(AiChatMessage {
                                        role: AiChatRole::Activity,
                                        content: String::new(),
                                        attachments: Vec::new(),
                                        step: Some(AiAgentStep::new(
                                            name.clone(),
                                            arguments.clone(),
                                            *is_mcp,
                                        )),
                                    });
                                },
                            );
                        }
                        AiUiEvent::StepEnd {
                            conv_id,
                            name,
                            output,
                            is_error,
                        } => {
                            mutate_conv_messages(
                                conversations,
                                messages,
                                active_id,
                                conv_id,
                                |msgs| {
                                    let target = msgs.iter_mut().rev().find(|m| {
                                        m.step.as_ref().is_some_and(|s| {
                                            s.name == *name
                                                && s.state == AiStepState::Running
                                        })
                                    });
                                    if let Some(msg) = target {
                                        if let Some(step) = msg.step.as_mut() {
                                            step.output = output.clone();
                                            step.state = if *is_error {
                                                AiStepState::Failed
                                            } else {
                                                AiStepState::Done
                                            };
                                        }
                                    }
                                },
                            );
                            let list: Vec<_> =
                                conversations.get_untracked().into_iter().collect();
                            let aid = active_id.get_untracked();
                            save_conversations(&list, &aid);
                        }
                        AiUiEvent::NeedApproval {
                            conv_id,
                            name,
                            arguments,
                            reply,
                        } => {
                            if active_id.get_untracked() == *conv_id {
                                pending_tool.set(Some(PendingToolApproval {
                                    name: name.clone(),
                                    arguments: arguments.clone(),
                                    reply: reply.clone(),
                                }));
                                status.set(format!(
                                    "Approve tool `{name}`? Run or Skip"
                                ));
                            } else {
                                // Auto-skip tools for background chats to avoid
                                // blocking the wrong conversation's approval UI.
                                let _ = reply.send(false);
                            }
                        }
                        AiUiEvent::SetBusy {
                            conv_id,
                            busy: is_busy,
                        } => {
                            if !*is_busy {
                                active_runs.update(|m| {
                                    m.remove(conv_id);
                                });
                                pending_tool.set(None);
                                let still =
                                    !active_runs.with_untracked(|m| m.is_empty());
                                busy.set(still);
                                let all = conversations.with_untracked(|list| {
                                    list.iter()
                                        .find(|c| c.id == *conv_id)
                                        .map(|c| c.messages.clone())
                                        .unwrap_or_default()
                                });
                                if !all.is_empty() {
                                    conversations.update(|list| {
                                        if let Some(c) = list
                                            .iter_mut()
                                            .find(|c| c.id == *conv_id)
                                        {
                                            c.title = derive_title(&all);
                                            c.updated_at = now_secs();
                                        }
                                    });
                                }
                                let list: Vec<_> = conversations
                                    .get_untracked()
                                    .into_iter()
                                    .collect();
                                let aid = active_id.get_untracked();
                                save_conversations(&list, &aid);
                                if !still && !queue.with_untracked(|q| q.is_empty())
                                {
                                    ai_self.drain_queue();
                                }
                            } else {
                                busy.set(true);
                            }
                        }
                    }
                }
            });
        });

        std::thread::spawn(move || {
            run_ai_ask(
                workspace,
                ai_cfg,
                mode,
                prompt,
                attachments,
                history,
                cancel,
                tx,
                run_conv_id,
            );
        });
    }
}

fn mutate_conv_messages(
    conversations: RwSignal<Vector<AiConversation>>,
    messages: RwSignal<Vector<AiChatMessage>>,
    active_id: RwSignal<String>,
    conv_id: &str,
    mutator: impl FnOnce(&mut Vec<AiChatMessage>),
) {
    let mut snapshot = None;
    conversations.update(|list| {
        if let Some(c) = list.iter_mut().find(|c| c.id == conv_id) {
            mutator(&mut c.messages);
            c.title = derive_title(&c.messages);
            c.updated_at = now_secs();
            snapshot = Some(c.messages.clone());
        }
    });
    if let Some(all) = snapshot {
        if active_id.get_untracked() == conv_id {
            messages.set(Vector::from(all));
        }
    }
}

fn derive_title(msgs: &[AiChatMessage]) -> String {
    msgs.iter()
        .find(|m| matches!(m.role, AiChatRole::User))
        .map(|m| {
            let t = m.content.lines().next().unwrap_or("New chat").trim();
            let mut s: String = t.chars().take(42).collect();
            if t.chars().count() > 42 {
                s.push('…');
            }
            if s.is_empty() { "New chat".into() } else { s }
        })
        .unwrap_or_else(|| "New chat".into())
}

/// Title to show for the active chat in the header dropdown.
fn active_title_from(list: &[AiConversation], active_id: &str) -> String {
    list.iter()
        .find(|c| c.id == active_id)
        .map(|c| c.title.clone())
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| "New chat".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation(id: &str, title: &str) -> AiConversation {
        AiConversation {
            id: id.into(),
            title: title.into(),
            mode: "agent".into(),
            model: "Auto".into(),
            messages: Vec::new(),
            updated_at: 0,
        }
    }

    #[test]
    fn active_title_prefers_the_active_conversation() {
        let list = vec![
            conversation("chat-1", "Fix the parser"),
            conversation("chat-2", "Add tests"),
        ];
        assert_eq!(active_title_from(&list, "chat-2"), "Add tests");
    }

    #[test]
    fn active_title_falls_back_when_missing_or_blank() {
        let list = vec![conversation("chat-1", "   ")];
        assert_eq!(active_title_from(&list, "chat-1"), "New chat");
        assert_eq!(active_title_from(&list, "chat-9"), "New chat");
        assert_eq!(active_title_from(&[], "chat-9"), "New chat");
    }

    #[test]
    fn derive_title_uses_the_first_user_line_and_truncates() {
        let msgs = vec![
            AiChatMessage::new(AiChatRole::Activity, "thinking".into()),
            AiChatMessage::new(
                AiChatRole::User,
                format!("{}\nsecond line", "x".repeat(60)),
            ),
        ];
        let title = derive_title(&msgs);
        assert!(title.ends_with('…'));
        assert_eq!(title.chars().count(), 43);
    }

    #[test]
    fn derive_title_defaults_without_user_messages() {
        assert_eq!(derive_title(&[]), "New chat");
    }
}

fn citation_label(path: &Path, workspace: Option<&Path>) -> String {
    let rel = workspace
        .and_then(|root| path.strip_prefix(root).ok())
        .unwrap_or(path);
    let display = rel.to_string_lossy();
    if display.is_empty() {
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string())
    } else {
        display.into_owned()
    }
}

fn classify_attachment(path: &Path) -> AiAttachmentKind {
    if path.is_dir() {
        return AiAttachmentKind::Directory;
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => AiAttachmentKind::Image,
        "pdf" | "doc" | "docx" | "odt" | "rtf" | "epub" => {
            AiAttachmentKind::Document
        }
        _ => AiAttachmentKind::Text,
    }
}

fn mime_for_image(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/jpeg",
    }
}

fn prepare_attachments(attachments: &[AiAttachment]) -> (String, Vec<String>) {
    let mut text_ctx = String::new();
    let mut images = Vec::new();

    for att in attachments {
        match att.kind {
            AiAttachmentKind::Image => {
                match std::fs::metadata(&att.path).and_then(|m| {
                    if m.len() > MAX_IMAGE_ATTACH_BYTES {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "image too large",
                        ));
                    }
                    std::fs::read(&att.path)
                }) {
                    Ok(bytes) => {
                        let b64 =
                            base64::engine::general_purpose::STANDARD.encode(&bytes);
                        let mime = mime_for_image(&att.path);
                        images.push(format!("data:{mime};base64,{b64}"));
                        text_ctx.push_str(&format!(
                            "\n[Image attached: {}]\n",
                            att.path.display()
                        ));
                    }
                    Err(e) => {
                        text_ctx.push_str(&format!(
                            "\n[Failed to read image {}: {e}]\n",
                            att.path.display()
                        ));
                    }
                }
            }
            AiAttachmentKind::Text | AiAttachmentKind::Document => {
                match std::fs::metadata(&att.path).and_then(|m| {
                    if m.len() > MAX_TEXT_ATTACH_BYTES {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "file too large",
                        ));
                    }
                    std::fs::read_to_string(&att.path)
                }) {
                    Ok(content) => {
                        text_ctx.push_str(&format!(
                            "\n----- attached file: {} -----\n{content}\n----- end -----\n",
                            att.path.display()
                        ));
                    }
                    Err(_) => {
                        // binary / unreadable: note path only
                        text_ctx.push_str(&format!(
                            "\n[Binary/unreadable attachment: {} — AI can use workspace tools to inspect if under the project root]\n",
                            att.path.display()
                        ));
                    }
                }
            }
            AiAttachmentKind::Directory => {
                text_ctx.push_str(&format!(
                    "\n[Directory citation: `{}` — use list_directory / search_code / get_project_structure under this path]\n",
                    att.name
                ));
            }
        }
    }

    (text_ctx, images)
}

// The ask pipeline threads workspace, provider config, agent mode, prompt,
// attachments, history, cancellation and the event sink. All are distinct
// concerns owned by the spawning thread; a parameter struct would just relocate
// the same fields without removing the coupling.
#[allow(clippy::too_many_arguments)]
fn run_ai_ask(
    workspace: Arc<LapceWorkspace>,
    ai_cfg: crate::config::ai::AiConfig,
    mode: AgentMode,
    prompt: String,
    attachments: Vec<AiAttachment>,
    history: Vec<(AiChatRole, String)>,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<AiUiEvent>,
    conv_id: String,
) {
    use std::time::Duration;

    use devforge_agent::{
        AgentEvent, AgentRequest, AgentRuntime, FsWorkspaceBackend, HistoryTurn,
        McpHub, McpServerSpec, ProviderConfig, Role, WorkspaceBackend,
    };

    let send = |ev: AiUiEvent| {
        let _ = tx.send(ev);
    };
    let cid = || conv_id.clone();

    let root = match workspace_root(&workspace) {
        Some(p) => p,
        None => {
            send(AiUiEvent::Append {
                conv_id: cid(),
                msg: AiChatMessage {
                    role: AiChatRole::System,
                    content: "Open a local folder workspace to use AI Ask tools."
                        .into(),
                    attachments: Vec::new(),
                    step: None,
                },
            });
            send(AiUiEvent::Status {
                conv_id: cid(),
                text: "Error".into(),
            });
            send(AiUiEvent::SetBusy {
                conv_id: cid(),
                busy: false,
            });
            return;
        }
    };

    let (attach_ctx, image_urls) = prepare_attachments(&attachments);
    let user_message = if attach_ctx.is_empty() {
        prompt
    } else if prompt.is_empty() {
        format!("Please review the attached materials.{attach_ctx}")
    } else {
        format!("{prompt}\n{attach_ctx}")
    };

    let extra_env_keys = provider_preset(&ai_cfg.provider)
        .map(|p| p.env_keys.iter().map(|s| (*s).to_string()).collect())
        .unwrap_or_default();

    let provider = ProviderConfig {
        base_url: ai_cfg.provider_base_url(),
        api_key: ai_cfg.resolved_api_key(),
        model: ai_cfg.model.clone(),
        temperature: ai_cfg.temperature,
        max_tokens: ai_cfg.max_tokens,
        extra_env_keys,
    };

    let runtime = match AgentRuntime::new(provider) {
        Ok(r) => r,
        Err(e) => {
            send(AiUiEvent::Append {
                conv_id: cid(),
                msg: AiChatMessage {
                    role: AiChatRole::System,
                    content: format!("Provider error: {e}"),
                    attachments: Vec::new(),
                    step: None,
                },
            });
            send(AiUiEvent::Status {
                conv_id: cid(),
                text: "Error".into(),
            });
            send(AiUiEvent::SetBusy {
                conv_id: cid(),
                busy: false,
            });
            return;
        }
    };

    let mcp = if ai_cfg.mcp_enabled {
        let mut specs: Vec<McpServerSpec> = ai_cfg
            .mcp_servers
            .iter()
            .filter(|s| s.enabled && !s.command.is_empty())
            .map(|s| McpServerSpec {
                name: s.name.clone(),
                command: s.command.clone(),
                args: s.args.clone(),
            })
            .collect();
        // Merge servers installed through the Agent Assets page.
        for (_, spec) in devforge_agent::mcp_asset_specs(Some(root.as_path())) {
            if specs.iter().any(|s| s.name == spec.name) {
                continue;
            }
            specs.push(spec);
        }
        if specs.is_empty() {
            None
        } else {
            Some(std::sync::Arc::new(McpHub::connect(&specs)))
        }
    } else {
        None
    };

    let backend = FsWorkspaceBackend::new(root);
    let history_turns: Vec<HistoryTurn> = history
        .into_iter()
        .filter_map(|(role, content)| {
            let role = match role {
                AiChatRole::User => Role::User,
                AiChatRole::Assistant => Role::Assistant,
                _ => return None,
            };
            Some(HistoryTurn { role, content })
        })
        .collect();

    let mcp_note = if mcp.is_some() {
        "MCP tools enabled."
    } else if ai_cfg.mcp_enabled {
        "MCP enabled but no servers configured in settings.toml [[ai.mcp-servers]]."
    } else {
        "MCP off."
    };

    let tools_note = match mode {
        AgentMode::Ask => "tools: read-only (+ optional MCP)",
        AgentMode::Edit => "tools: read + write_file/str_replace (+ optional MCP)",
        AgentMode::Agent => "tools: read + write + iterate (+ optional MCP)",
    };

    let request = AgentRequest {
        user_message,
        image_data_urls: image_urls,
        history: history_turns,
        context_preamble: format!(
            "Workspace root: {}\nUI mode: {} ({tools_note}).\n{mcp_note}\nProvider: {}.\nContinue the conversation using prior turns for context.",
            backend.root().display(),
            mode.as_str(),
            ai_cfg.provider,
        ),
        mode,
        max_iterations: ai_cfg.max_iterations.min(ai_cfg.tool_call_limit).max(1),
        cancel: cancel.clone(),
        require_tool_approval: ai_cfg.require_tool_approval,
        assets_enabled: ai_cfg.assets_enabled,
        hooks_enabled: ai_cfg.hooks_enabled,
        mcp,
    };

    let mut streaming = false;
    let mut emit = |ev: AgentEvent| match ev {
        AgentEvent::State(s) => send(AiUiEvent::Status {
            conv_id: cid(),
            text: s.to_string(),
        }),
        AgentEvent::Activity(a) => send(AiUiEvent::Append {
            conv_id: cid(),
            msg: AiChatMessage {
                role: AiChatRole::Activity,
                content: a,
                attachments: Vec::new(),
                step: None,
            },
        }),
        AgentEvent::TextDelta(t) => {
            if !streaming {
                send(AiUiEvent::BeginAssistant { conv_id: cid() });
                streaming = true;
            }
            send(AiUiEvent::AssistantDelta {
                conv_id: cid(),
                delta: t,
            });
        }
        AgentEvent::ToolStart { name, arguments } => {
            streaming = false;
            send(AiUiEvent::StepStart {
                conv_id: cid(),
                is_mcp: name.starts_with("mcp__"),
                name,
                arguments,
            });
        }
        AgentEvent::ToolEnd {
            name,
            output,
            is_error,
        } => {
            send(AiUiEvent::StepEnd {
                conv_id: cid(),
                name,
                output,
                is_error,
            });
        }
        AgentEvent::Error(e) => {
            send(AiUiEvent::Append {
                conv_id: cid(),
                msg: AiChatMessage {
                    role: AiChatRole::System,
                    content: format!("Error: {e}"),
                    attachments: Vec::new(),
                    step: None,
                },
            });
            send(AiUiEvent::Status {
                conv_id: cid(),
                text: "Error".into(),
            });
        }
        AgentEvent::Done => {}
    };

    let tx_approve = tx.clone();
    let approve_conv = conv_id.clone();
    let mut approve = move |name: &str, args: &str| -> bool {
        let (reply_tx, reply_rx) = mpsc::channel();
        let _ = tx_approve.send(AiUiEvent::NeedApproval {
            conv_id: approve_conv.clone(),
            name: name.to_string(),
            arguments: args.to_string(),
            reply: reply_tx,
        });
        reply_rx
            .recv_timeout(Duration::from_secs(600))
            .unwrap_or_default()
    };
    let _ = runtime.run_ask(&backend, request, &mut emit, &mut approve);
    send(AiUiEvent::SetBusy {
        conv_id: cid(),
        busy: false,
    });
}

fn workspace_root(workspace: &LapceWorkspace) -> Option<PathBuf> {
    match &workspace.kind {
        LapceWorkspaceType::Local => workspace.path.clone(),
        _ => None,
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_secs_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn history_path() -> Option<PathBuf> {
    Directory::config_directory().map(|d| d.join(HISTORY_FILE))
}

#[derive(Serialize, Deserialize)]
struct StoredAiHistory {
    active_id: String,
    conversations: Vec<AiConversation>,
}

fn load_conversations() -> Option<(Vec<AiConversation>, String)> {
    let path = history_path()?;
    let data = std::fs::read_to_string(path).ok()?;
    let stored: StoredAiHistory = serde_json::from_str(&data).ok()?;
    if stored.conversations.is_empty() {
        return None;
    }
    Some((stored.conversations, stored.active_id))
}

fn save_conversations(list: &[AiConversation], active_id: &str) {
    let Some(path) = history_path() else {
        return;
    };
    let stored = StoredAiHistory {
        active_id: active_id.to_string(),
        conversations: list.to_vec(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&stored) {
        let _ = std::fs::write(path, json);
    }
}

use std::{
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use floem::{
    ext_event::create_signal_from_channel,
    keyboard::Modifiers,
    reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith, create_effect},
};
use im::Vector;
use lapce_xi_rope::Rope;
use devforge_core::mode::Mode;

use crate::{
    command::{CommandExecuted, CommandKind},
    editor::EditorData,
    keypress::{KeyPressFocus, condition::Condition},
    main_split::Editors,
    window_tab::CommonData,
    workspace::{LapceWorkspace, LapceWorkspaceType},
};

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug)]
pub struct AiChatMessage {
    pub role: AiChatRole,
    pub content: String,
}

#[derive(Clone)]
pub struct AiData {
    pub common: Rc<CommonData>,
    pub query_editor: EditorData,
    pub messages: RwSignal<Vector<AiChatMessage>>,
    pub status: RwSignal<String>,
    pub busy: RwSignal<bool>,
    pub cancel: RwSignal<Arc<AtomicBool>>,
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
        matches!(condition, Condition::PanelFocus)
    }

    fn run_command(
        &self,
        command: &crate::command::LapceCommand,
        count: Option<usize>,
        mods: Modifiers,
    ) -> CommandExecuted {
        match &command.kind {
            CommandKind::Workbench(_) => {}
            CommandKind::Scroll(_) => {}
            CommandKind::Focus(_) => {}
            CommandKind::Edit(_)
            | CommandKind::Move(_)
            | CommandKind::MultiSelection(_) => {
                return self.query_editor.run_command(command, count, mods);
            }
            CommandKind::MotionMode(_) => {}
        }
        CommandExecuted::No
    }

    fn receive_char(&self, c: &str) {
        self.query_editor.receive_char(c);
    }
}

#[derive(Clone)]
enum AiUiEvent {
    Status(String),
    Append(AiChatMessage),
    SetBusy(bool),
}

impl AiData {
    pub fn new(cx: Scope, editors: Editors, common: Rc<CommonData>) -> Self {
        let query_editor = editors.make_local(cx, common.clone());
        Self {
            common,
            query_editor,
            messages: cx.create_rw_signal(Vector::new()),
            status: cx.create_rw_signal("Idle".into()),
            busy: cx.create_rw_signal(false),
            cancel: cx.create_rw_signal(Arc::new(AtomicBool::new(false))),
        }
    }

    pub fn stop(&self) {
        self.cancel.get_untracked().store(true, Ordering::SeqCst);
        self.status.set("Cancelled".into());
        self.busy.set(false);
    }

    pub fn send(&self, workspace: Arc<LapceWorkspace>) {
        if self.busy.get_untracked() {
            return;
        }
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
        if prompt.is_empty() {
            return;
        }

        let mut msgs = self.messages.get_untracked();
        msgs.push_back(AiChatMessage {
            role: AiChatRole::User,
            content: prompt.clone(),
        });
        self.messages.set(msgs);
        self.query_editor.doc().reload(Rope::from(""), true);

        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel.set(cancel.clone());
        self.busy.set(true);
        self.status.set("Analyzing".into());

        let messages = self.messages;
        let status = self.status;
        let busy = self.busy;
        let ai_cfg = config.ai.clone();

        let (tx, rx) = mpsc::channel::<AiUiEvent>();
        let notification = create_signal_from_channel(rx);
        create_effect(move |_| {
            notification.with(|ev| {
                if let Some(ev) = ev.as_ref() {
                    match ev {
                        AiUiEvent::Status(s) => status.set(s.clone()),
                        AiUiEvent::Append(msg) => {
                            let mut msgs = messages.get_untracked();
                            msgs.push_back(msg.clone());
                            messages.set(msgs);
                        }
                        AiUiEvent::SetBusy(b) => busy.set(*b),
                    }
                }
            });
        });

        std::thread::spawn(move || {
            run_ai_ask(workspace, ai_cfg, prompt, cancel, tx);
        });
    }
}

fn run_ai_ask(
    workspace: Arc<LapceWorkspace>,
    ai_cfg: crate::config::ai::AiConfig,
    prompt: String,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<AiUiEvent>,
) {
    use devforge_agent::{
        AgentEvent, AgentMode, AgentRequest, AgentRuntime, FsWorkspaceBackend,
        ProviderConfig, WorkspaceBackend,
    };

    let send = |ev: AiUiEvent| {
        let _ = tx.send(ev);
    };

    let root = match workspace_root(&workspace) {
        Some(p) => p,
        None => {
            send(AiUiEvent::Append(AiChatMessage {
                role: AiChatRole::System,
                content: "Open a local folder workspace to use AI Ask tools.".into(),
            }));
            send(AiUiEvent::Status("Error".into()));
            send(AiUiEvent::SetBusy(false));
            return;
        }
    };

    let provider = ProviderConfig {
        base_url: ai_cfg.provider_base_url(),
        api_key: ai_cfg.api_key.clone(),
        model: ai_cfg.model.clone(),
        temperature: ai_cfg.temperature,
        max_tokens: ai_cfg.max_tokens,
    };

    let runtime = match AgentRuntime::new(provider) {
        Ok(r) => r,
        Err(e) => {
            send(AiUiEvent::Append(AiChatMessage {
                role: AiChatRole::System,
                content: format!("Provider error: {e}"),
            }));
            send(AiUiEvent::Status("Error".into()));
            send(AiUiEvent::SetBusy(false));
            return;
        }
    };

    let backend = FsWorkspaceBackend::new(root);
    let request = AgentRequest {
        user_message: prompt,
        context_preamble: format!(
            "Workspace root: {}\nMode: ask (read-only tools only).",
            backend.root().display()
        ),
        mode: AgentMode::Ask,
        max_iterations: ai_cfg.max_iterations.min(ai_cfg.tool_call_limit).max(1),
        cancel,
    };

    let mut assistant = String::new();
    let mut emit = |ev: AgentEvent| match ev {
        AgentEvent::State(s) => send(AiUiEvent::Status(s.to_string())),
        AgentEvent::Activity(a) => send(AiUiEvent::Append(AiChatMessage {
            role: AiChatRole::Activity,
            content: a,
        })),
        AgentEvent::TextDelta(t) => {
            assistant.push_str(&t);
        }
        AgentEvent::ToolStart { name, arguments } => {
            send(AiUiEvent::Append(AiChatMessage {
                role: AiChatRole::Activity,
                content: format!("Tool `{name}` {arguments}"),
            }));
        }
        AgentEvent::ToolEnd { name, is_error } => {
            send(AiUiEvent::Append(AiChatMessage {
                role: AiChatRole::Activity,
                content: if is_error {
                    format!("Tool `{name}` failed")
                } else {
                    format!("Tool `{name}` done")
                },
            }));
        }
        AgentEvent::Error(e) => {
            send(AiUiEvent::Append(AiChatMessage {
                role: AiChatRole::System,
                content: format!("Error: {e}"),
            }));
            send(AiUiEvent::Status("Error".into()));
        }
        AgentEvent::Done => {}
    };

    let _ = runtime.run_ask(&backend, request, &mut emit);

    if !assistant.is_empty() {
        send(AiUiEvent::Append(AiChatMessage {
            role: AiChatRole::Assistant,
            content: assistant,
        }));
    }
    send(AiUiEvent::SetBusy(false));
}

fn workspace_root(workspace: &LapceWorkspace) -> Option<PathBuf> {
    match &workspace.kind {
        LapceWorkspaceType::Local => workspace.path.clone(),
        _ => None,
    }
}

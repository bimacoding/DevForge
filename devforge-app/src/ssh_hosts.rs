//! Remote SSH host picker backed by `~/.ssh/config` (Cursor-style).

use std::{path::PathBuf, rc::Rc, sync::Arc};

use devforge_core::{command::FocusCommand, mode::Mode};
use floem::{
    View,
    event::EventListener,
    keyboard::Modifiers,
    reactive::{RwSignal, Scope, SignalGet, SignalUpdate, SignalWith},
    style::{CursorStyle, Display, Position},
    views::{Decorators, container, dyn_stack, label, scroll, stack, text},
};

use crate::{
    command::{CommandExecuted, CommandKind, InternalCommand, WindowCommand},
    config::{LapceConfig, color::LapceColor},
    keypress::KeyPressFocus,
    main_split::Editors,
    ssh_config::{
        append_host_block, ensure_ssh_config_exists, read_ssh_config_hosts,
        ssh_config_display_path, ssh_config_path, ssh_host_template,
    },
    window_tab::{CommonData, Focus, WindowTabData},
    workspace::{LapceWorkspace, LapceWorkspaceType, SshHost},
};

#[derive(Clone)]
pub struct SshHostsData {
    pub visible: RwSignal<bool>,
    pub hosts: RwSignal<Vec<SshHost>>,
    pub config_path: RwSignal<Option<PathBuf>>,
    pub error: RwSignal<Option<String>>,
    pub focus: RwSignal<Focus>,
    pub internal_command: crate::listener::Listener<InternalCommand>,
}

impl std::fmt::Debug for SshHostsData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshHostsData")
            .field("visible", &self.visible.get_untracked())
            .finish()
    }
}

impl SshHostsData {
    pub fn new(
        cx: Scope,
        _editors: Editors,
        common: Rc<CommonData>,
        focus: RwSignal<Focus>,
    ) -> Self {
        Self {
            visible: cx.create_rw_signal(false),
            hosts: cx.create_rw_signal(Vec::new()),
            config_path: cx.create_rw_signal(None),
            error: cx.create_rw_signal(None),
            focus,
            internal_command: common.internal_command,
        }
    }

    pub fn open(&self) {
        self.reload();
        self.visible.set(true);
        self.focus.set(Focus::SshHostsPopup);
    }

    pub fn close(&self) {
        self.visible.set(false);
        self.error.set(None);
        self.focus.set(Focus::Workbench);
    }

    pub fn reload(&self) {
        match read_ssh_config_hosts() {
            Ok(hosts) => {
                self.hosts.set(hosts);
                self.error.set(None);
            }
            Err(err) => {
                self.hosts.set(Vec::new());
                self.error.set(Some(err.to_string()));
            }
        }
        self.config_path.set(
            ssh_config_path()
                .ok()
                .or_else(|| ensure_ssh_config_exists().ok()),
        );
    }

    pub fn open_config_file(&self) {
        match ensure_ssh_config_exists() {
            Ok(path) => {
                self.close();
                self.internal_command
                    .send(InternalCommand::OpenFile { path });
            }
            Err(err) => {
                self.error
                    .set(Some(format!("Cannot open ~/.ssh/config: {err}")));
            }
        }
    }

    pub fn add_host(&self) {
        match append_host_block(ssh_host_template()) {
            Ok(path) => {
                self.reload();
                self.close();
                self.internal_command
                    .send(InternalCommand::OpenFile { path });
            }
            Err(err) => {
                self.error
                    .set(Some(format!("Cannot update ~/.ssh/config: {err}")));
            }
        }
    }

    pub fn connect(
        &self,
        host: SshHost,
        window_command: crate::listener::Listener<WindowCommand>,
    ) {
        self.close();
        // Resolve $HOME on the remote so File Explorer opens the user's
        // home folder (Cursor-style) instead of an empty workspace.
        let path = crate::proxy::resolve_ssh_home(&host);
        window_command.send(WindowCommand::SetWorkspace {
            workspace: LapceWorkspace {
                kind: LapceWorkspaceType::RemoteSSH(host),
                path: Some(path),
                last_open: 0,
            },
        });
    }
}

impl KeyPressFocus for SshHostsData {
    fn get_mode(&self) -> Mode {
        Mode::Insert
    }

    fn check_condition(
        &self,
        _condition: crate::keypress::condition::Condition,
    ) -> bool {
        self.visible.get_untracked()
    }

    fn run_command(
        &self,
        command: &crate::command::LapceCommand,
        _count: Option<usize>,
        _mods: Modifiers,
    ) -> CommandExecuted {
        match &command.kind {
            CommandKind::Focus(FocusCommand::ModalClose) => {
                self.close();
                CommandExecuted::Yes
            }
            _ => CommandExecuted::Yes,
        }
    }

    fn receive_char(&self, _c: &str) {}

    fn focus_only(&self) -> bool {
        true
    }
}

pub fn ssh_hosts_popup(window_tab_data: Rc<WindowTabData>) -> impl View {
    let data = window_tab_data.ssh_hosts_data.clone();
    let config = window_tab_data.common.config;
    let window_command = window_tab_data.common.window_common.window_command;
    let visible = data.visible;
    let hosts = data.hosts;
    let error = data.error;
    let config_path = data.config_path;

    container(
        container(
            stack((
                label(|| "Remote SSH".to_string()).style(move |s| {
                    s.font_bold()
                        .font_size(18.0)
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),
                label(move || {
                    let path = config_path
                        .get()
                        .map(|p| ssh_config_display_path(&p))
                        .unwrap_or_else(|| "~/.ssh/config".to_string());
                    let count = hosts.with(|h| h.len());
                    format!(
                        "{count} host(s) from {path} — Connect, or edit the config file"
                    )
                })
                .style(move |s| {
                    s.margin_top(6.0)
                        .margin_bottom(14.0)
                        .color(config.get().color(LapceColor::EDITOR_DIM))
                }),
                scroll({
                    let data = data.clone();
                    dyn_stack(
                        move || hosts.get(),
                        |h| {
                            h.alias
                                .clone()
                                .unwrap_or_else(|| h.display_name())
                        },
                        move |host| {
                            host_row(host, data.clone(), window_command, config)
                        },
                    )
                    .style(|s| s.flex_col().width_full().row_gap(6.0))
                })
                .style(|s| s.width_full().max_height(360.0).min_height(100.0)),
                label(move || {
                    if hosts.with(|h| h.is_empty()) {
                        "No Host entries in ~/.ssh/config yet. Click Add Host."
                            .to_string()
                    } else {
                        String::new()
                    }
                })
                .style(move |s| {
                    s.margin_top(8.0)
                        .color(config.get().color(LapceColor::EDITOR_DIM))
                        .apply_if(!hosts.with(|h| h.is_empty()), |s| s.hide())
                }),
                label(move || error.get().unwrap_or_default()).style(move |s| {
                    s.margin_top(8.0)
                        .color(config.get().color(LapceColor::LAPCE_ERROR))
                        .apply_if(error.with(|e| e.is_none()), |s| s.hide())
                }),
                stack((
                    action_button(
                        config,
                        "Add Host",
                        {
                            let data = data.clone();
                            move || data.add_host()
                        },
                        || true,
                    ),
                    action_button(
                        config,
                        "Edit SSH Config",
                        {
                            let data = data.clone();
                            move || data.open_config_file()
                        },
                        || true,
                    ),
                    action_button(
                        config,
                        "Refresh",
                        {
                            let data = data.clone();
                            move || data.reload()
                        },
                        || true,
                    ),
                    action_button(
                        config,
                        "Close",
                        {
                            let data = data.clone();
                            move || data.close()
                        },
                        || true,
                    ),
                ))
                .style(|s| s.margin_top(16.0).col_gap(8.0).items_center()),
            ))
            .style(|s| {
                s.flex_col()
                    .items_center()
                    .min_width(440.0)
                    .max_width(620.0)
            }),
        )
        .style(move |s| {
            let config = config.get();
            s.padding_vert(24.0)
                .padding_horiz(28.0)
                .border(1.0)
                .border_radius(8.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .background(config.color(LapceColor::PANEL_BACKGROUND))
        })
        .on_event_stop(EventListener::PointerDown, |_| {}),
    )
    .on_event_stop(EventListener::PointerDown, {
        let data = data.clone();
        move |_| {
            data.close();
        }
    })
    .style(move |s| {
        s.display(if visible.get() {
            Display::Flex
        } else {
            Display::None
        })
        .position(Position::Absolute)
        .size_pct(100.0, 100.0)
        .flex_col()
        .items_center()
        .justify_center()
        .background(
            config
                .get()
                .color(LapceColor::LAPCE_DROPDOWN_SHADOW)
                .multiply_alpha(0.5),
        )
        .z_index(20)
    })
    .debug_name("SSH Hosts Popup")
}

fn host_row(
    host: SshHost,
    data: SshHostsData,
    window_command: crate::listener::Listener<WindowCommand>,
    config: floem::reactive::ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let title = host.display_name();
    let subtitle = {
        let mut parts = Vec::new();
        if let Some(user) = &host.user {
            parts.push(format!("{user}@{}", host.host));
        } else {
            parts.push(host.host.clone());
        }
        if let Some(port) = host.port {
            parts.push(format!(":{port}"));
        }
        if let Some(key) = &host.identity_file {
            parts.push(key.clone());
        }
        parts.join(" · ")
    };
    let connect_host = host.clone();
    let connect_host_row = host;

    stack((
        stack((
            label(move || title.clone()).style(move |s| {
                s.font_bold()
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            label(move || subtitle.clone()).style(move |s| {
                s.font_size(12.0)
                    .margin_top(2.0)
                    .color(config.get().color(LapceColor::EDITOR_DIM))
            }),
        ))
        .style(|s| s.flex_col().items_start().flex_grow(1.0).min_width(0.0)),
        label(|| "Connect".to_string())
            .style(move |s| {
                let config = config.get();
                s.padding_horiz(12.0)
                    .padding_vert(6.0)
                    .border_radius(4.0)
                    .color(config.color(LapceColor::EDITOR_FOREGROUND))
                    .background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
                    .cursor(CursorStyle::Pointer)
            })
            .on_click_stop({
                let data = data.clone();
                move |_| {
                    data.connect(connect_host.clone(), window_command);
                }
            }),
    ))
    .style(move |s| {
        let config = config.get();
        s.width_full()
            .padding(10.0)
            .border(1.0)
            .border_radius(6.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .items_center()
            .hover(|s| {
                s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
            })
    })
    .on_click_stop({
        let data = data.clone();
        move |_| {
            data.connect(connect_host_row.clone(), window_command);
        }
    })
}

fn action_button(
    config: floem::reactive::ReadSignal<Arc<LapceConfig>>,
    title: &'static str,
    on_click: impl Fn() + 'static,
    visible: impl Fn() -> bool + 'static,
) -> impl View {
    container(text(title))
        .on_click_stop(move |_| on_click())
        .style(move |s| {
            let config = config.get();
            s.padding_horiz(12.0)
                .padding_vert(8.0)
                .border(1.0)
                .border_radius(6.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
                .cursor(CursorStyle::Pointer)
                .apply_if(!visible(), |s| s.hide())
                .hover(|s| {
                    s.background(
                        config.color(LapceColor::PANEL_HOVERED_ACTIVE_BACKGROUND),
                    )
                })
        })
}

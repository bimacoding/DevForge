use std::rc::Rc;

use floem::{
    View,
    event::EventListener,
    reactive::{SignalGet, SignalWith},
    style::{AlignItems, CursorStyle, Display, JustifyContent, Position},
    views::{Decorators, container, label, stack, svg},
};

use crate::{
    command::LapceWorkbenchCommand,
    config::{color::LapceColor, icon::LapceIcons},
    window_tab::WindowTabData,
};

/// Start page shown when no folder/workspace is open and no editor tabs are active.
pub fn welcome_view(window_tab_data: Rc<WindowTabData>) -> impl View {
    let config = window_tab_data.common.config;
    let workbench_command = window_tab_data.common.workbench_command;
    let workspace = window_tab_data.workspace.clone();
    let editor_tabs = window_tab_data.main_split.editor_tabs;
    let panel = window_tab_data.panel.clone();
    let logo_size = 72.0;

    container(
        stack((
            svg(move || config.get().logo_svg()).style(move |s| {
                s.size(logo_size, logo_size)
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            label(|| "DevForge".to_string()).style(move |s| {
                s.font_bold()
                    .font_size(28.0)
                    .margin_top(16.0)
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            label(|| "Open a folder or start something new".to_string()).style(
                move |s| {
                    s.margin_top(8.0)
                        .margin_bottom(28.0)
                        .color(config.get().color(LapceColor::EDITOR_DIM))
                },
            ),
            stack((
                welcome_button(
                    config,
                    LapceIcons::DIRECTORY_OPENED,
                    "Open Folder",
                    "Open a local folder as workspace",
                    {
                        move || {
                            workbench_command
                                .send(LapceWorkbenchCommand::OpenFolder);
                        }
                    },
                ),
                welcome_button(
                    config,
                    LapceIcons::FILE_EXPLORER,
                    "Open Recent Workspace",
                    "Pick from recently opened workspaces",
                    {
                        move || {
                            workbench_command
                                .send(LapceWorkbenchCommand::OpenWorkspace);
                        }
                    },
                ),
                welcome_button(
                    config,
                    LapceIcons::ADD,
                    "New Project",
                    "Create or select a folder for a new project",
                    {
                        move || {
                            workbench_command
                                .send(LapceWorkbenchCommand::NewProject);
                        }
                    },
                ),
                welcome_button(
                    config,
                    LapceIcons::SCM,
                    "Clone Repository",
                    "Clone a git repository and open it",
                    {
                        move || {
                            workbench_command
                                .send(LapceWorkbenchCommand::CloneRepository);
                        }
                    },
                ),
                welcome_button(
                    config,
                    LapceIcons::REMOTE,
                    "Remote SSH",
                    "Connect using hosts from ~/.ssh/config",
                    {
                        move || {
                            workbench_command
                                .send(LapceWorkbenchCommand::ManageSshHosts);
                        }
                    },
                ),
                welcome_button(
                    config,
                    LapceIcons::FILE,
                    "New File",
                    "Start with an untitled buffer",
                    {
                        move || {
                            workbench_command.send(LapceWorkbenchCommand::NewFile);
                        }
                    },
                ),
                welcome_button(
                    config,
                    LapceIcons::WINDOW_MAXIMIZE,
                    "New Window",
                    "Open another DevForge window",
                    {
                        move || {
                            workbench_command.send(LapceWorkbenchCommand::NewWindow);
                        }
                    },
                ),
            ))
            .style(|s| {
                s.flex_col()
                    .align_items(AlignItems::Stretch)
                    .min_width(360.0)
                    .max_width(440.0)
                    .row_gap(8.0)
            }),
        ))
        .style(|s| s.flex_col().items_center().justify_center().padding(32.0)),
    )
    .style(move |s| {
        let has_workspace = workspace.path.is_some();
        let has_open_editors = editor_tabs.with(|tabs| {
            tabs.values()
                .any(|tab| tab.with(|t| !t.children.is_empty()))
        });
        // Don't cover a maximized bottom panel (Terminal, etc.)
        let bottom_maximized = panel.panel_bottom_maximized(true)
            && panel.is_container_shown(
                &crate::panel::position::PanelContainerPosition::Bottom,
                true,
            );
        let show = !has_workspace && !has_open_editors && !bottom_maximized;
        let config = config.get();
        s.display(if show { Display::Flex } else { Display::None })
            .position(Position::Absolute)
            .size_pct(100.0, 100.0)
            .items_center()
            .justify_content(Some(JustifyContent::Center))
            .background(config.color(LapceColor::EDITOR_BACKGROUND))
            .z_index(1)
            .apply_if(!show, |s| s.pointer_events_none())
    })
    .on_event_stop(EventListener::PointerDown, |_| {})
    .debug_name("Welcome View")
}

fn welcome_button(
    config: floem::reactive::ReadSignal<std::sync::Arc<crate::config::LapceConfig>>,
    icon: &'static str,
    title: &'static str,
    subtitle: &'static str,
    on_click: impl Fn() + 'static,
) -> impl View {
    container(
        stack((
            svg(move || config.get().ui_svg(icon)).style(move |s| {
                let config = config.get();
                let size = config.ui.icon_size() as f32 + 4.0;
                s.size(size, size)
                    .margin_right(12.0)
                    .color(config.color(LapceColor::LAPCE_ICON_ACTIVE))
            }),
            stack((
                label(move || title.to_string()).style(move |s| {
                    s.font_bold()
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),
                label(move || subtitle.to_string()).style(move |s| {
                    s.font_size(12.0)
                        .margin_top(2.0)
                        .color(config.get().color(LapceColor::EDITOR_DIM))
                }),
            ))
            .style(|s| s.flex_col().items_start()),
        ))
        .style(|s| s.items_center().width_full()),
    )
    .on_click_stop(move |_| {
        on_click();
    })
    .style(move |s| {
        let config = config.get();
        s.width_full()
            .padding_horiz(14.0)
            .padding_vert(12.0)
            .border(1.0)
            .border_radius(8.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .background(config.color(LapceColor::PANEL_BACKGROUND))
            .cursor(CursorStyle::Pointer)
            .hover(|s| {
                s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
            })
            .active(|s| {
                s.background(
                    config.color(LapceColor::PANEL_HOVERED_ACTIVE_BACKGROUND),
                )
            })
    })
}

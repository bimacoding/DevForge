use std::rc::Rc;

use floem::{
    IntoView, View,
    event::EventListener,
    peniko::kurbo::{Point, Size},
    reactive::{SignalGet, SignalUpdate, SignalWith, create_rw_signal},
    style::CursorStyle,
    views::{
        Decorators, container, dyn_stack, empty, label, rich_text, scroll::scroll,
        stack, svg, text,
    },
};

use super::{
    data::PanelSection, kind::PanelKind, position::PanelPosition, view::PanelBuilder,
};
use crate::{
    ai::{AiAttachmentKind, AiChatRole, AiData},
    app::clickable_icon,
    config::{color::LapceColor, icon::LapceIcons},
    markdown::{MarkdownContent, parse_markdown},
    text_input::TextInputBuilder,
    window_tab::{Focus, WindowTabData},
};

pub fn ai_panel(
    window_tab_data: Rc<WindowTabData>,
    position: PanelPosition,
) -> impl View {
    let config = window_tab_data.common.config;
    let open = window_tab_data.panel.section_open(PanelSection::AiChat);
    PanelBuilder::new(config, position)
        .add("AI Assistant", ai_panel_content(window_tab_data), open)
        .build()
        .debug_name("AI Panel")
}

fn ai_panel_content(window_tab_data: Rc<WindowTabData>) -> impl View {
    let ai = window_tab_data.ai.clone();
    let focus = window_tab_data.common.focus;
    let workspace = window_tab_data.workspace.clone();
    let is_focused = move || focus.get() == Focus::Panel(PanelKind::Ai);
    let cursor_x = create_rw_signal(0.0);

    let tabs = conversation_tabs(ai.clone());
    let messages_view = messages_list(ai.clone());
    let bottom = stack((
        tool_approval_bar(ai.clone()),
        queue_bar(ai.clone(), workspace.clone()),
        status_bar(ai.clone()),
        composer_box(ai.clone(), workspace, is_focused, cursor_x, focus),
    ))
    .style(|s| {
        s.flex_col()
            .width_pct(100.0)
            .flex_grow(0.0)
            .flex_shrink(0.0)
            .items_stretch()
    });

    stack((tabs, messages_view, bottom)).style(|s| {
        s.flex_col()
            .size_pct(100.0, 100.0)
            .min_height(0.0)
            .items_stretch()
    })
}

fn conversation_tabs(ai: AiData) -> impl View {
    let config = ai.common.config;
    let ai_new = ai.clone();
    let tabs = scroll({
        dyn_stack(
            move || {
                ai.conversations
                    .get()
                    .into_iter()
                    .map(|c| (c.id, c.title))
                    .collect::<Vec<_>>()
            },
            |(id, _)| id.clone(),
            {
                let ai = ai.clone();
                move |(id, title): (String, String)| {
                    let ai_sel = ai.clone();
                    let ai_del = ai.clone();
                    let id_sel = id.clone();
                    let id_active = id.clone();
                    let id_del = id.clone();
                    stack((
                        label(move || title.clone())
                            .on_click_stop({
                                let ai = ai_sel.clone();
                                move |_| {
                                    ai.select_conversation(&id_sel);
                                }
                            })
                            .style(move |s| {
                                let config = config.get();
                                let active = ai.active_id.get() == id_active;
                                s.padding_horiz(10.0)
                                    .padding_vert(5.0)
                                    .font_size((config.ui.font_size() as f32) * 0.9)
                                    .color(config.color(if active {
                                        LapceColor::EDITOR_FOREGROUND
                                    } else {
                                        LapceColor::EDITOR_DIM
                                    }))
                                    .background(if active {
                                        config.color(
                                            LapceColor::PANEL_CURRENT_BACKGROUND,
                                        )
                                    } else {
                                        floem::peniko::Color::TRANSPARENT
                                    })
                                    .border_radius(6.0)
                                    .cursor(CursorStyle::Pointer)
                                    .max_width(140.0)
                                    .text_ellipsis()
                            }),
                        clickable_icon(
                            || LapceIcons::CLOSE,
                            {
                                let ai = ai_del;
                                move || {
                                    ai.delete_conversation(&id_del);
                                }
                            },
                            || false,
                            || false,
                            || "Close chat",
                            config,
                        ),
                    ))
                    .style(|s| s.items_center().gap(2.0))
                }
            },
        )
        .style(|s| s.flex_row().items_center().gap(4.0).padding_horiz(4.0))
    })
    .scroll_style(|s| s.hide_bars(true))
    .style(|s| s.flex_grow(1.0).min_width(0.0).height(34.0));

    let new_btn = clickable_icon(
        || LapceIcons::ADD,
        move || {
            ai_new.new_conversation();
        },
        || false,
        || false,
        || "New chat",
        config,
    );

    stack((tabs, new_btn)).style(move |s| {
        let config = config.get();
        s.width_pct(100.0)
            .flex_grow(0.0)
            .flex_shrink(0.0)
            .padding_horiz(6.0)
            .padding_vert(4.0)
            .items_center()
            .gap(4.0)
            .border_bottom(1.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
    })
}

fn messages_list(ai: AiData) -> impl View {
    let config = ai.common.config;
    scroll({
        dyn_stack(
            move || {
                ai.messages
                    .get()
                    .into_iter()
                    .enumerate()
                    .map(|(i, m)| (i, m))
                    .collect::<Vec<_>>()
            },
            |(i, m)| (*i, m.role.key(), m.content.len()),
            move |(_i, msg)| {
                let is_user = matches!(msg.role, AiChatRole::User);
                let is_activity = matches!(msg.role, AiChatRole::Activity);
                let is_system = matches!(msg.role, AiChatRole::System);
                let is_assistant = matches!(msg.role, AiChatRole::Assistant);
                let label_text = match msg.role {
                    AiChatRole::User => "You",
                    AiChatRole::Assistant => "DevForge",
                    AiChatRole::System => "System",
                    AiChatRole::Activity => "Tool",
                };
                let content = msg.content.clone();
                let attaches = msg.attachments.clone();

                let body = if is_assistant {
                    let md = parse_markdown(&content, 1.55, &config.get());
                    dyn_stack(
                        move || md.clone(),
                        |c| match c {
                            MarkdownContent::Text(t) => {
                                format!("t:{}", t.lines().len())
                            }
                            MarkdownContent::Image { url, .. } => {
                                format!("i:{url}")
                            }
                            MarkdownContent::Separator => "sep".into(),
                        },
                        move |content| match content {
                            MarkdownContent::Text(text_layout) => container(
                                rich_text(move || text_layout.clone())
                                    .style(|s| s.width_pct(100.0)),
                            )
                            .style(|s| s.width_pct(100.0))
                            .into_any(),
                            MarkdownContent::Image { .. } => empty().into_any(),
                            MarkdownContent::Separator => {
                                container(empty().style(move |s| {
                                    s.width_pct(100.0)
                                        .margin_vert(6.0)
                                        .height(1.0)
                                        .background(
                                            config
                                                .get()
                                                .color(LapceColor::LAPCE_BORDER),
                                        )
                                }))
                                .into_any()
                            }
                        },
                    )
                    .style(|s| s.flex_col().width_pct(100.0))
                    .into_any()
                } else {
                    text(content)
                        .style(move |s| {
                            let config = config.get();
                            s.color(config.color(LapceColor::EDITOR_FOREGROUND))
                                .font_size(config.ui.font_size() as f32)
                                .line_height(1.55)
                        })
                        .into_any()
                };

                container(
                    stack((
                        label(move || label_text.to_string()).style(move |s| {
                            let config = config.get();
                            s.font_bold()
                                .font_size((config.ui.font_size() as f32) * 0.8)
                                .color(config.color(LapceColor::EDITOR_DIM))
                                .margin_bottom(4.0)
                        }),
                        body,
                        if attaches.is_empty() {
                            empty().into_any()
                        } else {
                            dyn_stack(
                                move || attaches.clone(),
                                |name| name.clone(),
                                move |name| {
                                    label(move || format!("@{name}")).style(
                                        move |s| {
                                            let config = config.get();
                                            s.margin_top(6.0)
                                                .margin_right(6.0)
                                                .padding_horiz(8.0)
                                                .padding_vert(2.0)
                                                .border_radius(8.0)
                                                .border(1.0)
                                                .border_color(
                                                    config.color(
                                                        LapceColor::LAPCE_BORDER,
                                                    ),
                                                )
                                                .color(
                                                    config.color(
                                                        LapceColor::EDITOR_DIM,
                                                    ),
                                                )
                                                .font_size(
                                                    (config.ui.font_size() as f32)
                                                        * 0.85,
                                                )
                                        },
                                    )
                                },
                            )
                            .style(|s| {
                                s.flex_row()
                                    .flex_wrap(floem::style::FlexWrap::Wrap)
                                    .margin_top(4.0)
                            })
                            .into_any()
                        },
                    ))
                    .style(|s| s.flex_col().max_width_pct(92.0)),
                )
                .style(move |s| {
                    let config = config.get();
                    s.padding_horiz(12.0)
                        .padding_vert(10.0)
                        .margin_vert(4.0)
                        .margin_horiz(8.0)
                        .border_radius(12.0)
                        .items_start()
                        .apply_if(is_user, |s| {
                            s.margin_left_pct(8.0).background(
                                config.color(LapceColor::PANEL_CURRENT_BACKGROUND),
                            )
                        })
                        .apply_if(is_assistant, |s| {
                            s.margin_right_pct(8.0)
                                .border(1.0)
                                .border_color(config.color(LapceColor::LAPCE_BORDER))
                                .background(
                                    config.color(LapceColor::PANEL_BACKGROUND),
                                )
                        })
                        .apply_if(is_activity, |s| {
                            s.padding_vert(4.0)
                                .background(floem::peniko::Color::TRANSPARENT)
                        })
                        .apply_if(is_system, |s| {
                            s.border(1.0)
                                .border_color(config.color(LapceColor::LAPCE_WARN))
                        })
                })
            },
        )
        .style(|s| {
            s.flex_col()
                .width_pct(100.0)
                .padding_vert(8.0)
                .padding_bottom(16.0)
        })
    })
    .style(|s| {
        s.width_pct(100.0)
            .flex_col()
            .flex_grow(1.0)
            .flex_basis(0.0)
            .flex_shrink(1.0)
            .min_height(0.0)
    })
}

fn tool_approval_bar(ai: AiData) -> impl View {
    let config = ai.common.config;
    let ai_run = ai.clone();
    let ai_skip = ai.clone();
    stack((
        label(move || {
            ai.pending_tool
                .get()
                .map(|p| {
                    let args = if p.arguments.len() > 120 {
                        format!("{}…", &p.arguments[..120])
                    } else {
                        p.arguments
                    };
                    format!("MCP / tool `{}`\n{args}", p.name)
                })
                .unwrap_or_default()
        })
        .style(move |s| {
            s.flex_grow(1.0)
                .min_width(0.0)
                .font_size((config.get().ui.font_size() as f32) * 0.85)
                .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
        }),
        label(|| "Run".to_string())
            .on_click_stop(move |_| {
                ai_run.approve_pending_tool(true);
            })
            .style(move |s| {
                let config = config.get();
                s.padding_horiz(12.0)
                    .padding_vert(6.0)
                    .border_radius(6.0)
                    .cursor(CursorStyle::Pointer)
                    .background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
                    .color(config.color(LapceColor::EDITOR_FOREGROUND))
            }),
        label(|| "Skip".to_string())
            .on_click_stop(move |_| {
                ai_skip.approve_pending_tool(false);
            })
            .style(move |s| {
                let config = config.get();
                s.padding_horiz(12.0)
                    .padding_vert(6.0)
                    .border_radius(6.0)
                    .cursor(CursorStyle::Pointer)
                    .border(1.0)
                    .border_color(config.color(LapceColor::LAPCE_BORDER))
                    .color(config.color(LapceColor::EDITOR_DIM))
            }),
    ))
    .style(move |s| {
        let visible = ai.pending_tool.with(|p| p.is_some());
        let config = config.get();
        s.width_pct(100.0)
            .items_center()
            .gap(8.0)
            .padding_horiz(10.0)
            .padding_vert(8.0)
            .flex_grow(0.0)
            .flex_shrink(0.0)
            .border_top(1.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .background(config.color(LapceColor::PANEL_CURRENT_BACKGROUND))
            .apply_if(!visible, |s| s.hide())
    })
}

fn queue_bar(
    ai: AiData,
    workspace: std::sync::Arc<crate::workspace::LapceWorkspace>,
) -> impl View {
    let config = ai.common.config;
    let ai_clear = ai.clone();
    stack((
        label(move || {
            let n = ai.queue.with(|q| q.len());
            format!("Queue ({n})")
        })
        .style(move |s| {
            s.font_bold()
                .padding_left(10.0)
                .color(config.get().color(LapceColor::EDITOR_DIM))
                .font_size((config.get().ui.font_size() as f32) * 0.85)
        }),
        scroll({
            let ai = ai.clone();
            let workspace = workspace.clone();
            dyn_stack(
                move || {
                    ai.queue
                        .get()
                        .into_iter()
                        .map(|q| {
                            (q.id, q.prompt.chars().take(40).collect::<String>())
                        })
                        .collect::<Vec<_>>()
                },
                |(id, _)| id.clone(),
                {
                    let ai = ai.clone();
                    let workspace = workspace.clone();
                    move |(id, preview): (String, String)| {
                        let ai_run = ai.clone();
                        let ai_skip = ai.clone();
                        let ws = workspace.clone();
                        let id_run = id.clone();
                        let id_skip = id.clone();
                        stack((
                            label(move || preview.clone()).style(move |s| {
                                s.max_width(160.0)
                                    .text_ellipsis()
                                    .font_size(
                                        (config.get().ui.font_size() as f32) * 0.85,
                                    )
                                    .color(
                                        config
                                            .get()
                                            .color(LapceColor::EDITOR_FOREGROUND),
                                    )
                            }),
                            label(|| "Run now".to_string())
                                .on_click_stop(move |_| {
                                    ai_run.run_queued_now(&id_run, ws.clone());
                                })
                                .style(move |s| {
                                    s.padding_horiz(8.0)
                                        .padding_vert(3.0)
                                        .border_radius(4.0)
                                        .cursor(CursorStyle::Pointer)
                                        .background(config.get().color(
                                            LapceColor::PANEL_HOVERED_BACKGROUND,
                                        ))
                                        .font_size(
                                            (config.get().ui.font_size() as f32)
                                                * 0.8,
                                        )
                                }),
                            label(|| "Skip".to_string())
                                .on_click_stop(move |_| {
                                    ai_skip.skip_queued(&id_skip);
                                })
                                .style(move |s| {
                                    s.padding_horiz(8.0)
                                        .padding_vert(3.0)
                                        .cursor(CursorStyle::Pointer)
                                        .color(
                                            config
                                                .get()
                                                .color(LapceColor::EDITOR_DIM),
                                        )
                                        .font_size(
                                            (config.get().ui.font_size() as f32)
                                                * 0.8,
                                        )
                                }),
                        ))
                        .style(move |s| {
                            s.items_center()
                                .gap(6.0)
                                .padding_horiz(8.0)
                                .padding_vert(4.0)
                                .border(1.0)
                                .border_radius(8.0)
                                .border_color(
                                    config.get().color(LapceColor::LAPCE_BORDER),
                                )
                        })
                    }
                },
            )
            .style(|s| s.flex_row().items_center().gap(6.0).padding_horiz(6.0))
        })
        .scroll_style(|s| s.hide_bars(true))
        .style(|s| s.flex_grow(1.0).height(36.0)),
        label(|| "Clear".to_string())
            .on_click_stop(move |_| {
                ai_clear.clear_queue();
            })
            .style(move |s| {
                s.padding_horiz(10.0)
                    .cursor(CursorStyle::Pointer)
                    .color(config.get().color(LapceColor::EDITOR_DIM))
                    .font_size((config.get().ui.font_size() as f32) * 0.85)
            }),
    ))
    .style(move |s| {
        let visible = ai.queue.with(|q| !q.is_empty());
        let config = config.get();
        s.width_pct(100.0)
            .items_center()
            .gap(4.0)
            .padding_vert(4.0)
            .flex_grow(0.0)
            .flex_shrink(0.0)
            .border_top(1.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .apply_if(!visible, |s| s.hide())
    })
}

fn status_bar(ai: AiData) -> impl View {
    let config = ai.common.config;
    label(move || {
        let busy = if ai.busy.get() { " • running" } else { "" };
        let listen = if ai.listening.get() {
            " • listening"
        } else {
            ""
        };
        format!("{}{busy}{listen}", ai.status.get())
    })
    .style(move |s| {
        let config = config.get();
        s.padding_horiz(8.0)
            .padding_vert(4.0)
            .flex_grow(0.0)
            .flex_shrink(0.0)
            .color(config.color(LapceColor::EDITOR_DIM))
            .font_size((config.ui.font_size() as f32) * 0.9)
    })
}

fn composer_box(
    ai: AiData,
    workspace: std::sync::Arc<crate::workspace::LapceWorkspace>,
    is_focused: impl Fn() -> bool + 'static + Copy,
    cursor_x: floem::reactive::RwSignal<f64>,
    focus: floem::reactive::RwSignal<Focus>,
) -> impl View {
    let config = ai.common.config;
    let editor = ai.query_editor.clone();

    let attach_chips = {
        let ai = ai.clone();
        dyn_stack(
            move || {
                ai.attachments
                    .get()
                    .into_iter()
                    .map(|a| (a.path, a.name, a.kind))
                    .collect::<Vec<_>>()
            },
            |(path, _, _)| path.clone(),
            {
                let ai = ai.clone();
                move |(path, name, kind): (
                    std::path::PathBuf,
                    String,
                    AiAttachmentKind,
                )| {
                    let icon = match kind {
                        AiAttachmentKind::Image => "🖼",
                        AiAttachmentKind::Document => "📄",
                        AiAttachmentKind::Text => "📝",
                        AiAttachmentKind::Directory => "📁",
                    };
                    let ai_rm = ai.clone();
                    let path_rm = path.clone();
                    stack((
                        label(move || format!("{icon} @{name}")).style(move |s| {
                            let config = config.get();
                            s.font_size((config.ui.font_size() as f32) * 0.85)
                                .color(config.color(LapceColor::EDITOR_FOREGROUND))
                                .padding_horiz(6.0)
                                .max_width(220.0)
                                .text_ellipsis()
                        }),
                        clickable_icon(
                            || LapceIcons::CLOSE,
                            move || {
                                ai_rm.remove_attachment(&path_rm);
                            },
                            || false,
                            || false,
                            || "Remove",
                            config,
                        ),
                    ))
                    .style(move |s| {
                        let config = config.get();
                        s.items_center()
                            .border(1.0)
                            .border_color(config.color(LapceColor::LAPCE_BORDER))
                            .border_radius(12.0)
                            .padding_left(2.0)
                            .padding_right(2.0)
                            .background(
                                config.color(LapceColor::PANEL_HOVERED_BACKGROUND),
                            )
                    })
                }
            },
        )
        .style(move |s| {
            s.flex_row()
                .flex_wrap(floem::style::FlexWrap::Wrap)
                .gap(6.0)
                .padding_horiz(8.0)
                .padding_top(6.0)
                .apply_if(ai.attachments.with(|a| a.is_empty()), |s| s.hide())
        })
    };

    let input = container({
        let ai_focus = ai.clone();
        scroll(
            TextInputBuilder::new()
                .is_focused(is_focused)
                .key_focus(ai_focus)
                .build_editor(editor)
                .placeholder(|| {
                    "Message DevForge AI…  Enter send · Shift+Enter newline"
                        .to_string()
                })
                .on_cursor_pos(move |point| {
                    cursor_x.set(point.x);
                })
                .style(|s| {
                    s.padding_vert(10.0)
                        .padding_horiz(12.0)
                        .min_width_pct(100.0)
                        .min_height(56.0)
                }),
        )
        .ensure_visible(move || {
            Size::new(20.0, 0.0)
                .to_rect()
                .with_origin(Point::new(cursor_x.get(), 0.0))
        })
        .on_event_cont(EventListener::PointerDown, move |_| {
            focus.set(Focus::Panel(PanelKind::Ai));
        })
        .scroll_style(|s| s.hide_bars(true))
        .style(|s| {
            s.width_pct(100.0)
                .min_height(64.0)
                .max_height(160.0)
                .cursor(CursorStyle::Text)
        })
    })
    .style(|s| s.width_pct(100.0));

    let toolbar = composer_toolbar(ai.clone(), workspace);

    stack((attach_chips, input, toolbar)).style(move |s| {
        let config = config.get();
        s.width_pct(100.0)
            .flex_col()
            .flex_grow(0.0)
            .flex_shrink(0.0)
            .margin(8.0)
            .padding_bottom(6.0)
            .border(1.0)
            .border_radius(12.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .background(config.color(LapceColor::EDITOR_BACKGROUND))
    })
}

fn composer_toolbar(
    ai: AiData,
    workspace: std::sync::Arc<crate::workspace::LapceWorkspace>,
) -> impl View {
    let config = ai.common.config;

    let mode_btn = {
        let ai_m = ai.clone();
        stack((
            label(move || format!("∞ {}", ai.mode.get().label())).style(move |s| {
                let config = config.get();
                s.font_size((config.ui.font_size() as f32) * 0.9)
                    .color(config.color(LapceColor::EDITOR_FOREGROUND))
                    .padding_left(8.0)
                    .padding_vert(4.0)
            }),
            svg(move || config.get().ui_svg(LapceIcons::DROPDOWN_ARROW)).style(
                move |s| {
                    let config = config.get();
                    let size = (config.ui.icon_size() as f32) * 0.85;
                    s.size(size, size)
                        .color(config.color(LapceColor::EDITOR_DIM))
                        .margin_right(6.0)
                },
            ),
        ))
        .on_click_stop(move |_| {
            ai_m.show_mode_menu();
        })
        .style(move |s| {
            let config = config.get();
            s.items_center()
                .border_radius(8.0)
                .cursor(CursorStyle::Pointer)
                .hover(|s| {
                    s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
                })
        })
    };

    let model_btn = {
        let ai_m = ai.clone();
        stack((
            label(move || {
                let m = ai.model.get();
                if m.is_empty() || m.eq_ignore_ascii_case("auto") {
                    "Auto".into()
                } else {
                    m
                }
            })
            .style(move |s| {
                let config = config.get();
                s.font_size((config.ui.font_size() as f32) * 0.9)
                    .color(config.color(LapceColor::EDITOR_DIM))
                    .padding_left(8.0)
                    .padding_vert(4.0)
            }),
            svg(move || config.get().ui_svg(LapceIcons::DROPDOWN_ARROW)).style(
                move |s| {
                    let config = config.get();
                    let size = (config.ui.icon_size() as f32) * 0.85;
                    s.size(size, size)
                        .color(config.color(LapceColor::EDITOR_DIM))
                        .margin_right(6.0)
                },
            ),
        ))
        .on_click_stop(move |_| {
            ai_m.show_model_menu();
        })
        .style(move |s| {
            let config = config.get();
            s.items_center()
                .border_radius(8.0)
                .cursor(CursorStyle::Pointer)
                .hover(|s| {
                    s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
                })
        })
    };

    let provider_btn = {
        let ai_p = ai.clone();
        let config_sig = config;
        stack((
            label(move || {
                let p = config_sig.get().ai.provider.clone();
                format!("◈ {p}")
            })
            .style(move |s| {
                let config = config.get();
                s.font_size((config.ui.font_size() as f32) * 0.85)
                    .color(config.color(LapceColor::EDITOR_DIM))
                    .padding_left(8.0)
                    .padding_vert(4.0)
                    .max_width(120.0)
                    .text_ellipsis()
            }),
            svg(move || config.get().ui_svg(LapceIcons::DROPDOWN_ARROW)).style(
                move |s| {
                    let config = config.get();
                    let size = (config.ui.icon_size() as f32) * 0.85;
                    s.size(size, size)
                        .color(config.color(LapceColor::EDITOR_DIM))
                        .margin_right(6.0)
                },
            ),
        ))
        .on_click_stop(move |_| {
            ai_p.show_provider_menu();
        })
        .style(move |s| {
            let config = config.get();
            s.items_center()
                .border_radius(8.0)
                .cursor(CursorStyle::Pointer)
                .hover(|s| {
                    s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
                })
        })
    };

    let left = stack((mode_btn, model_btn, provider_btn))
        .style(|s| s.items_center().gap(4.0));

    let attach_btn = {
        let ai_a = ai.clone();
        clickable_icon(
            || LapceIcons::FILE,
            move || {
                ai_a.pick_attachments();
            },
            || false,
            || false,
            || "Attach file / image / document",
            config,
        )
    };

    let mic_btn = {
        let ai_s = ai.clone();
        clickable_icon(
            || LapceIcons::KEYBOARD,
            move || {
                ai_s.toggle_speech();
            },
            {
                let ai = ai.clone();
                move || ai.listening.get()
            },
            || false,
            || "Speech to text (system dictation)",
            config,
        )
    };

    let send_or_stop = {
        let ai_send = ai.clone();
        let ai_stop = ai.clone();
        let ws = workspace.clone();
        container(
            svg(move || {
                let config = config.get();
                if ai.busy.get() {
                    config.ui_svg(LapceIcons::DEBUG_STOP)
                } else {
                    config.ui_svg(LapceIcons::START)
                }
            })
            .style(move |s| {
                let config = config.get();
                let size = config.ui.icon_size() as f32;
                s.size(size, size)
                    .color(config.color(LapceColor::LAPCE_BUTTON_PRIMARY_FOREGROUND))
            }),
        )
        .on_click_stop(move |_| {
            if ai_stop.busy.get_untracked() {
                ai_stop.stop();
            } else {
                ai_send.send(ws.clone());
            }
        })
        .style(move |s| {
            let config = config.get();
            s.padding(6.0)
                .border_radius(999.0)
                .cursor(CursorStyle::Pointer)
                .background(
                    config.color(LapceColor::LAPCE_BUTTON_PRIMARY_BACKGROUND),
                )
                .hover(|s| {
                    s.background(
                        config.color(LapceColor::PANEL_HOVERED_ACTIVE_BACKGROUND),
                    )
                })
        })
    };

    let right = stack((attach_btn, mic_btn, send_or_stop))
        .style(|s| s.items_center().gap(2.0).padding_right(6.0));

    stack((left, empty().style(|s| s.flex_grow(1.0)), right)).style(|s| {
        s.width_pct(100.0)
            .items_center()
            .padding_horiz(4.0)
            .padding_top(2.0)
    })
}

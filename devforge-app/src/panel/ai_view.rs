//! Cursor-style AI chat panel: bubble transcript, observable agent/MCP steps,
//! a compact history header, mode/model pickers, and a compact composer.

use std::{rc::Rc, sync::Arc};

use floem::{
    AnyView, IntoView, View,
    event::EventListener,
    peniko::kurbo::{Point, Size},
    reactive::{
        ReadSignal, RwSignal, SignalGet, SignalUpdate, SignalWith, create_rw_signal,
    },
    style::{CursorStyle, FlexWrap},
    views::{
        Decorators, container, dyn_stack, editor::core::register::Clipboard,
        editor::text::SystemClipboard, empty, label, rich_text, scroll::scroll,
        stack, svg, text,
    },
};

use super::{
    data::PanelSection, kind::PanelKind, position::PanelPosition, view::PanelBuilder,
};
use crate::{
    ai::{
        AiAttachmentKind, AiChatMessage, AiChatRole, AiData, AiStepState,
        step_summary,
    },
    app::clickable_icon,
    config::{LapceConfig, color::LapceColor, icon::LapceIcons},
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

    let messages_view = messages_list(ai.clone());
    let bottom = stack((
        tool_approval_bar(ai.clone()),
        queue_bar(ai.clone(), workspace.clone()),
        composer_box(ai.clone(), workspace, is_focused, cursor_x, focus),
    ))
    .style(|s| {
        s.flex_col()
            .width_pct(100.0)
            .flex_grow(0.0)
            .flex_shrink(0.0)
            .items_stretch()
    });

    stack((
        chat_header(ai.clone()),
        messages_view,
        touched_files_footer(ai.clone(), window_tab_data.clone()),
        bottom,
    ))
    .style(|s| {
        s.flex_col()
            .size_pct(100.0, 100.0)
            .min_height(0.0)
            .items_stretch()
    })
}

/// Compact chat header (Cursor-style): active chat title doubling as the
/// history dropdown, plus new-chat and history actions.
fn chat_header(ai: AiData) -> impl View {
    let config = ai.common.config;
    let ai_new = ai.clone();
    let ai_hist = ai.clone();
    let ai_title = ai.clone();

    let title_btn = stack((
        svg(move || config.get().ui_svg(LapceIcons::AI_SPARKLE)).style(move |s| {
            let config = config.get();
            let size = (config.ui.icon_size() as f32) * 0.85;
            s.size(size, size)
                .color(config.color(LapceColor::EDITOR_DIM))
        }),
        label(move || ai_title.active_title()).style(move |s| {
            let config = config.get();
            s.font_size((config.ui.font_size() as f32) * 0.9)
                .color(config.color(LapceColor::EDITOR_FOREGROUND))
                .max_width(190.0)
                .text_ellipsis()
        }),
        svg(move || config.get().ui_svg(LapceIcons::DROPDOWN_ARROW)).style(
            move |s| {
                let config = config.get();
                let size = (config.ui.icon_size() as f32) * 0.75;
                s.size(size, size)
                    .color(config.color(LapceColor::EDITOR_DIM))
            },
        ),
    ))
    .on_click_stop({
        let ai_hist = ai_hist.clone();
        move |_| {
            ai_hist.show_history_menu();
        }
    })
    .style(move |s| {
        let config = config.get();
        s.items_center()
            .gap(6.0)
            .flex_grow(1.0)
            .min_width(0.0)
            .padding_horiz(8.0)
            .padding_vert(4.0)
            .border_radius(8.0)
            .cursor(CursorStyle::Pointer)
            .hover(|s| {
                s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
            })
    });

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

    let history_btn = clickable_icon(
        || LapceIcons::AI_HISTORY,
        move || {
            ai_hist.show_history_menu();
        },
        || false,
        || false,
        || "Chat history",
        config,
    );

    stack((title_btn, new_btn, history_btn)).style(move |s| {
        let config = config.get();
        s.width_pct(100.0)
            .flex_grow(0.0)
            .flex_shrink(0.0)
            .padding_horiz(6.0)
            .padding_vert(3.0)
            .items_center()
            .gap(2.0)
            .border_bottom(1.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
    })
}

fn messages_list(ai: AiData) -> impl View {
    let config = ai.common.config;
    let workspace = ai.common.workspace.clone();
    scroll({
        dyn_stack(
            move || {
                ai.messages
                    .get()
                    .into_iter()
                    .enumerate()
                    .collect::<Vec<_>>()
            },
            |(i, m)| (*i, m.role.key(), m.content.len()),
            move |(_i, msg)| {
                message_bubble(msg, ai.clone(), workspace.clone(), config)
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

fn message_bubble(
    msg: AiChatMessage,
    ai: AiData,
    workspace: Arc<crate::workspace::LapceWorkspace>,
    config: ReadSignal<Arc<LapceConfig>>,
) -> AnyView {
    match msg.role {
        AiChatRole::Activity => match msg.step.clone() {
            Some(step) => agent_step_bubble(step, config).into_any(),
            None => activity_bubble(msg.content.clone(), config).into_any(),
        },
        AiChatRole::User => user_bubble(msg, config).into_any(),
        AiChatRole::Assistant => {
            assistant_bubble(msg.content.clone(), ai, workspace, config).into_any()
        }
        AiChatRole::System => system_bubble(msg.content.clone(), config).into_any(),
    }
}

fn user_bubble(
    msg: AiChatMessage,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let attaches = msg.attachments.clone();
    container(
        stack((
            text(msg.content).style(move |s| {
                let config = config.get();
                s.color(config.color(LapceColor::EDITOR_FOREGROUND))
                    .font_size(config.ui.font_size() as f32)
                    .line_height(1.55)
            }),
            attachment_chips(attaches, config),
        ))
        .style(|s| s.flex_col().max_width_pct(100.0)),
    )
    .style(move |s| {
        let config = config.get();
        s.padding_horiz(12.0)
            .padding_vert(10.0)
            .margin_vert(4.0)
            .margin_horiz(8.0)
            .margin_left_pct(10.0)
            .border_radius(12.0)
            .background(config.color(LapceColor::PANEL_CURRENT_BACKGROUND))
    })
}

fn assistant_bubble(
    content: String,
    ai: AiData,
    workspace: Arc<crate::workspace::LapceWorkspace>,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let hovered = create_rw_signal(false);
    let actions = assistant_actions(content.clone(), ai, workspace, hovered, config);
    let md = parse_markdown(&content, 1.55, &config.get());
    let body = dyn_stack(
        move || md.clone(),
        |c| match c {
            MarkdownContent::Text(t) => format!("t:{}", t.lines().len()),
            MarkdownContent::Image { url, .. } => format!("i:{url}"),
            MarkdownContent::Separator => "sep".into(),
        },
        move |content| match content {
            MarkdownContent::Text(text_layout) => container(
                rich_text(move || text_layout.clone()).style(|s| s.width_pct(100.0)),
            )
            .style(|s| s.width_pct(100.0))
            .into_any(),
            MarkdownContent::Image { .. } => empty().into_any(),
            MarkdownContent::Separator => container(empty().style(move |s| {
                s.width_pct(100.0)
                    .margin_vert(6.0)
                    .height(1.0)
                    .background(config.get().color(LapceColor::LAPCE_BORDER))
            }))
            .into_any(),
        },
    )
    .style(|s| s.flex_col().width_pct(100.0));

    container(
        stack((body, actions))
            .style(|s| s.flex_col().width_pct(100.0).items_start()),
    )
    .on_event_stop(EventListener::PointerEnter, move |_| hovered.set(true))
    .on_event_stop(EventListener::PointerLeave, move |_| hovered.set(false))
    .style(move |s| {
        let config = config.get();
        s.padding_horiz(12.0)
            .padding_vert(10.0)
            .margin_vert(4.0)
            .margin_horiz(8.0)
            .margin_right_pct(4.0)
            .border_radius(12.0)
            .border(1.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .background(config.color(LapceColor::PANEL_BACKGROUND))
    })
}

/// Copy + Retry actions, revealed on hover like Cursor's message toolbar.
fn assistant_actions(
    content: String,
    ai: AiData,
    workspace: Arc<crate::workspace::LapceWorkspace>,
    hovered: RwSignal<bool>,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let copy_payload = content;
    let copy_btn = icon_button(
        || LapceIcons::AI_COPY,
        move || {
            if !copy_payload.is_empty() {
                SystemClipboard::new().put_string(copy_payload.clone());
            }
        },
        || false,
        || false,
        || "Copy response",
        config,
    );
    let retry_btn = icon_button(
        || LapceIcons::AI_SPARKLE,
        move || {
            ai.retry_last(workspace.clone());
        },
        || false,
        || false,
        || "Retry",
        config,
    );

    container(stack((copy_btn, retry_btn)).style(|s| s.items_center().gap(2.0)))
        .style(move |s| {
            s.margin_top(6.0)
                .flex_row()
                .items_center()
                .apply_if(!hovered.get(), |s| s.hide())
        })
}

fn system_bubble(
    content: String,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    container(text(content).style(move |s| {
        let config = config.get();
        s.color(config.color(LapceColor::EDITOR_DIM))
            .font_size((config.ui.font_size() as f32) * 0.9)
            .line_height(1.5)
    }))
    .style(move |s| {
        let config = config.get();
        s.padding_horiz(10.0)
            .padding_vert(6.0)
            .margin_vert(4.0)
            .margin_horiz(8.0)
            .border_radius(8.0)
            .border(1.0)
            .border_color(config.color(LapceColor::LAPCE_WARN))
    })
}

/// Plain activity note (no tool metadata), e.g. "Model request (round 2)…".
fn activity_bubble(
    content: String,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    label(move || content.clone()).style(move |s| {
        let config = config.get();
        s.padding_horiz(10.0)
            .padding_vert(2.0)
            .margin_vert(1.0)
            .font_size((config.ui.font_size() as f32) * 0.85)
            .color(config.color(LapceColor::EDITOR_DIM))
    })
}

/// Collapsible agent/MCP step: icon + verb + files, expanding into
/// arguments and tool output (Cursor shows the same structure).
fn agent_step_bubble(
    step: crate::ai::AiAgentStep,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let open = create_rw_signal(false);
    let step_for_summary = step.clone();
    let args = Rc::new(step.arguments.clone());
    let output = Rc::new(step.output.clone());
    let files = step.files.clone();
    let state = step.state;
    let is_mcp = step.is_mcp;

    let (state_icon, state_key) = match state {
        AiStepState::Running => (LapceIcons::AI_SPARKLE, "running"),
        AiStepState::Done => (LapceIcons::AI_CHECK, "done"),
        AiStepState::Failed => (LapceIcons::ERROR, "failed"),
        AiStepState::Skipped => (LapceIcons::AI_DISABLED, "skipped"),
    };

    let header = stack((
        svg(move || {
            config.get().ui_svg(if open.get() {
                LapceIcons::AI_FOLD_OPEN
            } else {
                LapceIcons::AI_FOLD_CLOSED
            })
        })
        .style(move |s| {
            let size = (config.get().ui.icon_size() as f32) * 0.75;
            s.size(size, size)
                .color(config.get().color(LapceColor::EDITOR_DIM))
        }),
        svg(move || config.get().ui_svg(state_icon)).style(move |s| {
            let config = config.get();
            let size = (config.ui.icon_size() as f32) * 0.85;
            let color = match state_key {
                "failed" => config.color(LapceColor::LAPCE_ERROR),
                "running" => config.color(LapceColor::EDITOR_FOREGROUND),
                _ => config.color(LapceColor::EDITOR_DIM),
            };
            s.size(size, size).color(color)
        }),
        svg(move || {
            config.get().ui_svg(if is_mcp {
                LapceIcons::AI_MCP
            } else {
                LapceIcons::AI_TOOL
            })
        })
        .style(move |s| {
            let config = config.get();
            let size = (config.ui.icon_size() as f32) * 0.8;
            s.size(size, size)
                .color(config.color(LapceColor::EDITOR_DIM))
        }),
        label(move || step_summary(&step_for_summary)).style(move |s| {
            let config = config.get();
            s.font_size((config.ui.font_size() as f32) * 0.87)
                .color(config.color(LapceColor::EDITOR_FOREGROUND))
                .max_width_pct(80.0)
                .text_ellipsis()
        }),
    ))
    .on_click_stop(move |_| open.update(|o| *o = !*o))
    .style(|s| {
        s.items_center()
            .gap(6.0)
            .padding_vert(2.0)
            .cursor(CursorStyle::Pointer)
    });

    let details = stack((
        detail_block(
            {
                let args = args.clone();
                move || args.as_ref().clone()
            },
            "(no arguments)",
            {
                let args = args.clone();
                move || {
                    let trimmed = args.trim();
                    !trimmed.is_empty() && trimmed != "{}"
                }
            },
            config,
        ),
        detail_block(
            {
                let output = output.clone();
                move || output.as_ref().clone()
            },
            "No output captured",
            {
                let output = output.clone();
                move || !output.trim().is_empty()
            },
            config,
        ),
        file_chips(files, config),
    ))
    .style(move |s| {
        s.flex_col()
            .width_pct(100.0)
            .padding_left(22.0)
            .padding_top(4.0)
            .gap(4.0)
            .apply_if(!open.get(), |s| s.hide())
    });

    container(stack((header, details)).style(|s| s.flex_col().width_pct(100.0)))
        .style(move |s| {
            let config = config.get();
            s.width_pct(100.0)
                .padding_horiz(12.0)
                .padding_vert(4.0)
                .margin_vert(1.0)
                .margin_horiz(8.0)
                .border_radius(8.0)
                .border(1.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .background(config.color(LapceColor::PANEL_CURRENT_BACKGROUND))
        })
}

/// Monospace, scrollable block used for tool arguments and output.
fn detail_block(
    content: impl Fn() -> String + 'static,
    empty_hint: &'static str,
    visible: impl Fn() -> bool + 'static,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    container(
        scroll(
            label(move || {
                let c = content();
                if c.trim().is_empty() {
                    empty_hint.to_string()
                } else {
                    c
                }
            })
            .style(move |s| {
                let config = config.get();
                s.font_family(config.editor.font_family.clone())
                    .font_size((config.editor.font_size() as f32) * 0.86)
                    .color(config.color(LapceColor::EDITOR_DIM))
                    .line_height(1.4)
            }),
        )
        .scroll_style(|s| s.hide_bars(true))
        .style(|s| s.max_height(180.0)),
    )
    .style(move |s| {
        let config = config.get();
        s.width_pct(100.0)
            .padding_horiz(8.0)
            .padding_vert(6.0)
            .border_radius(6.0)
            .background(config.color(LapceColor::EDITOR_BACKGROUND))
            .apply_if(!visible(), |s| s.hide())
    })
}

fn file_chips(
    files: Vec<String>,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    container(
        dyn_stack(
            move || files.clone(),
            |f| f.clone(),
            move |file| {
                label(move || file.clone()).style(move |s| {
                    let config = config.get();
                    s.padding_horiz(6.0)
                        .padding_vert(1.0)
                        .border_radius(4.0)
                        .font_size((config.ui.font_size() as f32) * 0.82)
                        .color(config.color(LapceColor::EDITOR_FOREGROUND))
                        .background(
                            config.color(LapceColor::PANEL_HOVERED_BACKGROUND),
                        )
                        .max_width(220.0)
                        .text_ellipsis()
                })
            },
        )
        .style(|s| s.flex_row().flex_wrap(FlexWrap::Wrap).gap(4.0)),
    )
    .style(|s| s.width_pct(100.0))
}

fn attachment_chips(
    attaches: Vec<String>,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    dyn_stack(
        move || attaches.clone(),
        |name| name.clone(),
        move |name| {
            label(move || format!("@{name}")).style(move |s| {
                let config = config.get();
                s.margin_top(6.0)
                    .margin_right(6.0)
                    .padding_horiz(8.0)
                    .padding_vert(2.0)
                    .border_radius(8.0)
                    .border(1.0)
                    .border_color(config.color(LapceColor::LAPCE_BORDER))
                    .color(config.color(LapceColor::EDITOR_DIM))
                    .font_size((config.ui.font_size() as f32) * 0.85)
            })
        },
    )
    .style(|s| s.flex_row().flex_wrap(FlexWrap::Wrap).margin_top(4.0))
}

/// "N Files" label for the transcript footer ("" when nothing was touched).
fn format_touched_count(n: usize) -> String {
    match n {
        0 => String::new(),
        1 => "1 File".to_string(),
        n => format!("{n} Files"),
    }
}

/// Platform-appropriate shortcut hint shown next to the footer Stop action.
fn stop_shortcut() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘⌫"
    } else {
        "Ctrl+⌫"
    }
}

/// Icon button that fills its background on hover (Cursor-like toolbar button).
fn icon_button<S: std::fmt::Display + 'static>(
    icon: impl Fn() -> &'static str + 'static,
    on_click: impl Fn() + 'static,
    active_fn: impl Fn() -> bool + 'static,
    disabled_fn: impl Fn() -> bool + 'static + Copy,
    tooltip_: impl Fn() -> S + 'static + Clone,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    let view = container(svg(move || config.get().ui_svg(icon())).style(move |s| {
        let config = config.get();
        let size = (config.ui.icon_size() as f32) * 0.9;
        s.size(size, size)
            .color(config.color(LapceColor::EDITOR_DIM))
            .apply_if(active_fn(), |s| {
                s.color(config.color(LapceColor::EDITOR_FOREGROUND))
            })
    }))
    .disabled(disabled_fn)
    .on_click_stop(move |_| on_click())
    .style(move |s| {
        let config = config.get();
        s.padding(4.0)
            .border_radius(6.0)
            .cursor(CursorStyle::Pointer)
            .hover(|s| {
                s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
            })
    });

    crate::app::tooltip_label(config, view, tooltip_)
}

/// Footer under the transcript: files touched this conversation, plus the
/// Stop (while running) and Review actions — Cursor shows "N Files · Stop · Review".
fn touched_files_footer(
    ai: AiData,
    window_tab_data: Rc<WindowTabData>,
) -> impl View {
    let config = ai.common.config;
    let ai_count = ai.clone();
    let ai_stop = ai.clone();
    let ws_root = window_tab_data.workspace.path.clone();

    let stop_btn = stack((
        label(|| "Stop".to_string()).style(move |s| {
            let config = config.get();
            s.font_size((config.ui.font_size() as f32) * 0.82)
                .color(config.color(LapceColor::EDITOR_FOREGROUND))
        }),
        label(|| stop_shortcut().to_string()).style(move |s| {
            let config = config.get();
            s.font_size((config.ui.font_size() as f32) * 0.78)
                .color(config.color(LapceColor::EDITOR_DIM))
                .margin_left(4.0)
        }),
    ))
    .on_click_stop(move |_| {
        ai_stop.stop();
    })
    .style(move |s| {
        let config = config.get();
        s.items_center()
            .padding_horiz(8.0)
            .padding_vert(2.0)
            .border_radius(6.0)
            .cursor(CursorStyle::Pointer)
            .hover(|s| {
                s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
            })
            .apply_if(!ai.busy.get(), |s| s.hide())
    });

    let review_btn = {
        let ai_review = ai.clone();
        label(|| "Review".to_string())
            .on_click_stop(move |_| {
                let files = ai_review.touched_files();
                let Some(first) = files.first() else {
                    return;
                };
                let path = match &ws_root {
                    Some(root) => root.join(first),
                    None => std::path::PathBuf::from(first),
                };
                window_tab_data.main_split.open_file_changes(path);
            })
            .style(move |s| {
                let config = config.get();
                s.padding_horiz(8.0)
                    .padding_vert(2.0)
                    .border_radius(6.0)
                    .font_size((config.ui.font_size() as f32) * 0.82)
                    .color(config.color(LapceColor::EDITOR_FOREGROUND))
                    .cursor(CursorStyle::Pointer)
                    .hover(|s| {
                        s.background(
                            config.color(LapceColor::PANEL_HOVERED_BACKGROUND),
                        )
                    })
            })
    };

    stack((
        label(move || format_touched_count(ai_count.touched_files().len())).style(
            move |s| {
                let config = config.get();
                s.font_bold()
                    .font_size((config.ui.font_size() as f32) * 0.82)
                    .color(config.color(LapceColor::EDITOR_DIM))
            },
        ),
        empty().style(|s| s.flex_grow(1.0)),
        stop_btn,
        review_btn,
    ))
    .style(move |s| {
        let visible = !ai.touched_files().is_empty() || ai.busy.get();
        let config = config.get();
        s.width_pct(100.0)
            .items_center()
            .gap(6.0)
            .padding_horiz(10.0)
            .padding_vert(3.0)
            .flex_grow(0.0)
            .flex_shrink(0.0)
            .border_top(1.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .apply_if(!visible, |s| s.hide())
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
                .flex_wrap(FlexWrap::Wrap)
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
                .placeholder(|| "Add a follow-up".to_string())
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
            .background(config.color(LapceColor::AI_COMPOSER_BACKGROUND))
    })
}

fn composer_toolbar(
    ai: AiData,
    workspace: std::sync::Arc<crate::workspace::LapceWorkspace>,
) -> impl View {
    let config = ai.common.config;

    let mode_btn = {
        let ai_m = ai.clone();
        mode_pill(
            move || ai.mode.get().label().to_string(),
            move || ai_m.show_mode_menu(),
            config,
        )
    };

    let model_btn = {
        let ai_m = ai.clone();
        plain_dropdown(
            move || {
                let m = ai.model.get();
                if m.is_empty() || m.eq_ignore_ascii_case("auto") {
                    "Auto".into()
                } else {
                    m
                }
            },
            move || ai_m.show_model_menu(),
            config,
        )
    };

    let left = stack((mode_btn, model_btn)).style(|s| s.items_center().gap(2.0));

    let attach_btn = {
        let ai_a = ai.clone();
        icon_button(
            || LapceIcons::AI_ATTACH,
            move || {
                ai_a.pick_attachments();
            },
            || false,
            || false,
            || "Attach file / image / document",
            config,
        )
    };

    let manage_btn = {
        let ai_a = ai.clone();
        icon_button(
            || LapceIcons::AI_SPARKLE,
            move || {
                ai_a.open_agent_assets();
            },
            || false,
            || false,
            || "Manage Skills, MCPs, Subagents, Rules, Commands, Hooks",
            config,
        )
    };

    let mic_btn = {
        let ai_s = ai.clone();
        icon_button(
            || LapceIcons::AI_MIC,
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
                    config.ui_svg(LapceIcons::AI_STOP)
                } else {
                    config.ui_svg(LapceIcons::AI_SEND)
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

    let right = stack((attach_btn, manage_btn, mic_btn, send_or_stop))
        .style(|s| s.items_center().gap(2.0).padding_right(6.0));

    stack((left, empty().style(|s| s.flex_grow(1.0)), right)).style(|s| {
        s.width_pct(100.0)
            .items_center()
            .padding_horiz(4.0)
            .padding_top(2.0)
    })
}

/// Filled pill used for the composer's mode picker (Cursor's "Agent" chip).
fn mode_pill(
    text_fn: impl Fn() -> String + 'static,
    on_click: impl Fn() + 'static,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    stack((
        svg(move || config.get().ui_svg(LapceIcons::AI_SPARKLE)).style(move |s| {
            let config = config.get();
            let size = (config.ui.icon_size() as f32) * 0.75;
            s.size(size, size)
                .color(config.color(LapceColor::EDITOR_FOREGROUND))
                .margin_left(8.0)
        }),
        label(text_fn).style(move |s| {
            let config = config.get();
            s.font_size((config.ui.font_size() as f32) * 0.86)
                .color(config.color(LapceColor::EDITOR_FOREGROUND))
                .max_width(120.0)
                .text_ellipsis()
        }),
        svg(move || config.get().ui_svg(LapceIcons::DROPDOWN_ARROW)).style(
            move |s| {
                let config = config.get();
                let size = (config.ui.icon_size() as f32) * 0.72;
                s.size(size, size)
                    .color(config.color(LapceColor::EDITOR_DIM))
                    .margin_right(6.0)
            },
        ),
    ))
    .on_click_stop(move |_| on_click())
    .style(move |s| {
        let config = config.get();
        s.items_center()
            .gap(4.0)
            .padding_vert(3.0)
            .border_radius(8.0)
            .cursor(CursorStyle::Pointer)
            .background(config.color(LapceColor::PANEL_CURRENT_BACKGROUND))
            .hover(|s| {
                s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
            })
    })
}

/// Plain text + chevron picker used for the composer's model menu.
fn plain_dropdown(
    text_fn: impl Fn() -> String + 'static,
    on_click: impl Fn() + 'static,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    stack((
        label(text_fn).style(move |s| {
            let config = config.get();
            s.font_size((config.ui.font_size() as f32) * 0.86)
                .color(config.color(LapceColor::EDITOR_DIM))
                .padding_left(8.0)
                .padding_vert(4.0)
                .max_width(220.0)
                .text_ellipsis()
        }),
        svg(move || config.get().ui_svg(LapceIcons::DROPDOWN_ARROW)).style(
            move |s| {
                let config = config.get();
                let size = (config.ui.icon_size() as f32) * 0.72;
                s.size(size, size)
                    .color(config.color(LapceColor::EDITOR_DIM))
                    .margin_right(6.0)
            },
        ),
    ))
    .on_click_stop(move |_| on_click())
    .style(move |s| {
        let config = config.get();
        s.items_center()
            .border_radius(8.0)
            .cursor(CursorStyle::Pointer)
            .hover(|s| {
                s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
            })
    })
}

#[cfg(test)]
mod tests {
    use crate::ai::{AiAgentStep, AiStepState, step_summary, tool_verb};

    use super::format_touched_count;

    #[test]
    fn touched_count_label_matches_cursor_wording() {
        assert_eq!(format_touched_count(0), "");
        assert_eq!(format_touched_count(1), "1 File");
        assert_eq!(format_touched_count(3), "3 Files");
    }

    #[test]
    fn tool_verbs_are_human_readable() {
        assert_eq!(tool_verb("read_file"), "Read file");
        assert_eq!(tool_verb("str_replace"), "Edit file");
        assert_eq!(tool_verb("write_file"), "Write file");
        assert_eq!(tool_verb("mcp__mysql__execute_query"), "Run query");
        assert_eq!(tool_verb("mcp__mysql__list_tables"), "List tables");
        assert_eq!(tool_verb("unknown_tool"), "Tool call");
    }

    #[test]
    fn step_summary_includes_touched_file() {
        let step = AiAgentStep::new(
            "str_replace".into(),
            r#"{"path":"src/main.rs","old_string":"a","new_string":"b"}"#.into(),
            false,
        );
        assert_eq!(step.files, vec!["src/main.rs".to_string()]);
        assert_eq!(step_summary(&step), "Edit file · src/main.rs");
    }

    #[test]
    fn step_summary_without_path_is_just_the_verb() {
        let step = AiAgentStep::new(
            "search_code".into(),
            r#"{"query":"flake"}"#.into(),
            false,
        );
        assert!(step.files.is_empty());
        assert_eq!(step_summary(&step), "Search code");
    }

    #[test]
    fn steps_start_in_running_state() {
        let step = AiAgentStep::new("read_file".into(), "{}".into(), true);
        assert_eq!(step.state, AiStepState::Running);
        assert!(step.is_mcp);
        assert!(step.output.is_empty());
    }
}

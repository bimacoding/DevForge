use std::rc::Rc;

use floem::{
    View,
    event::EventListener,
    peniko::kurbo::{Point, Size},
    reactive::{SignalGet, SignalUpdate, create_rw_signal},
    style::CursorStyle,
    views::{
        Decorators, container, dyn_stack, label, scroll::scroll, stack, text,
    },
};

use super::{
    data::PanelSection, kind::PanelKind, position::PanelPosition, view::PanelBuilder,
};
use crate::{
    ai::AiChatRole,
    config::color::LapceColor,
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
    let config = window_tab_data.common.config;
    let focus = window_tab_data.common.focus;
    let workspace = window_tab_data.workspace.clone();
    let is_focused = move || focus.get() == Focus::Panel(PanelKind::Ai);
    let cursor_x = create_rw_signal(0.0);

    let messages_view = {
        let ai = ai.clone();
        scroll({
            dyn_stack(
                move || ai.messages.get(),
                |m| (m.role.key(), m.content.clone()),
                move |msg| {
                    let (prefix, color_key) = match msg.role {
                        AiChatRole::User => ("You", LapceColor::EDITOR_FOREGROUND),
                        AiChatRole::Assistant => {
                            ("AI", LapceColor::EDITOR_FOREGROUND)
                        }
                        AiChatRole::System => ("System", LapceColor::LAPCE_WARN),
                        AiChatRole::Activity => ("…", LapceColor::EDITOR_DIM),
                    };
                    let content = msg.content.clone();
                    stack((
                        label(move || prefix.to_string()).style(move |s| {
                            let config = config.get();
                            s.font_bold()
                                .color(config.color(color_key))
                                .font_size(config.ui.font_size() as f32)
                        }),
                        text(content).style(move |s| {
                            let config = config.get();
                            s.color(config.color(LapceColor::EDITOR_FOREGROUND))
                                .font_size(config.ui.font_size() as f32)
                                .line_height(1.5)
                        }),
                    ))
                    .style(|s| {
                        s.flex_col()
                            .width_pct(100.0)
                            .padding_vert(6.0)
                            .padding_horiz(8.0)
                            .gap(4.0)
                    })
                },
            )
            .style(|s| s.flex_col().width_pct(100.0).padding_bottom(8.0))
        })
        .style(|s| s.flex_col().flex_grow(1.0).min_height(0.0).width_pct(100.0))
    };

    let status = {
        let ai = ai.clone();
        label(move || {
            let busy = if ai.busy.get() { " • running" } else { "" };
            format!("{}{busy}", ai.status.get())
        })
        .style(move |s| {
            let config = config.get();
            s.padding_horiz(8.0)
                .padding_vert(4.0)
                .color(config.color(LapceColor::EDITOR_DIM))
                .font_size((config.ui.font_size() as f32) * 0.9)
        })
    };

    let editor = ai.query_editor.clone();
    let input = container({
        scroll(
            TextInputBuilder::new()
                .is_focused(is_focused)
                .build_editor(editor)
                .placeholder(|| "Ask AI about this project…".to_string())
                .on_cursor_pos(move |point| {
                    cursor_x.set(point.x);
                })
                .style(|s| s.padding_vert(4.0).padding_horiz(8.0).min_width_pct(100.0)),
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
        .style(move |s| {
            let config = config.get();
            s.width_pct(100.0)
                .height(52.0)
                .cursor(CursorStyle::Text)
                .items_center()
                .background(config.color(LapceColor::EDITOR_BACKGROUND))
                .border(1.0)
                .border_radius(6.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
        })
    })
    .style(|s| s.flex_grow(1.0).min_width(0.0));

    let send_btn = {
        let ai = ai.clone();
        let workspace = workspace.clone();
        label(|| "Send".to_string())
            .on_click_stop(move |_| {
                ai.send(workspace.clone());
            })
            .style(move |s| {
                let config = config.get();
                s.padding_horiz(10.0)
                    .padding_vert(6.0)
                    .margin_left(6.0)
                    .border_radius(4.0)
                    .background(
                        config.color(LapceColor::LAPCE_BUTTON_PRIMARY_BACKGROUND),
                    )
                    .color(
                        config.color(LapceColor::LAPCE_BUTTON_PRIMARY_FOREGROUND),
                    )
                    .cursor(CursorStyle::Pointer)
            })
    };

    let stop_btn = {
        let ai_stop = ai.clone();
        label(|| "Stop".to_string())
            .on_click_stop(move |_| {
                ai_stop.stop();
            })
            .style(move |s| {
                let config = config.get();
                s.padding_horiz(10.0)
                    .padding_vert(6.0)
                    .margin_left(6.0)
                    .border_radius(4.0)
                    .border(1.0)
                    .border_color(config.color(LapceColor::LAPCE_BORDER))
                    .color(config.color(LapceColor::EDITOR_FOREGROUND))
                    .cursor(CursorStyle::Pointer)
                    .apply_if(!ai.busy.get(), |s| s.hide())
            })
    };

    let input_row = stack((input, send_btn, stop_btn)).style(move |s| {
        let config = config.get();
        s.width_pct(100.0)
            .padding(8.0)
            .items_center()
            .border_top(1.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
    });

    stack((messages_view, status, input_row))
        .style(|s| s.flex_col().size_pct(100.0, 100.0).items_stretch())
}

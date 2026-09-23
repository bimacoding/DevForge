use std::{collections::BTreeMap, rc::Rc, sync::Arc, time::Duration};

use devforge_core::{buffer::rope_text::RopeText, mode::Mode};
use devforge_rpc::plugin::VoltID;
use floem::{
    AnyView, IntoView, View,
    action::{TimerToken, add_overlay, exec_after, remove_overlay},
    event::EventListener,
    keyboard::Modifiers,
    peniko::kurbo::{Point, Rect, Size},
    reactive::{
        Memo, ReadSignal, RwSignal, Scope, SignalGet, SignalUpdate, SignalWith,
        create_effect, create_memo, create_rw_signal,
    },
    style::{CursorStyle, MarginLeft, Transition},
    text::{Attrs, AttrsList, FamilyOwned, TextLayout},
    views::{
        Decorators, VirtualVector, container, dyn_stack, empty, label,
        scroll::{PropagatePointerWheel, scroll},
        stack, stack_from_iter, svg, text, virtual_stack,
    },
};
use indexmap::IndexMap;
use inflector::Inflector;
use lapce_xi_rope::Rope;
use serde::Serialize;
use std::time::Duration as StdDuration;

use crate::{
    ai_providers::McpServerConfig,
    command::{CommandExecuted, LapceWorkbenchCommand},
    config::{
        DropdownInfo, LapceConfig, ai::AiConfig, color::LapceColor,
        core::CoreConfig, editor::EditorConfig, icon::LapceIcons,
        terminal::TerminalConfig, ui::UIConfig,
    },
    keypress::KeyPressFocus,
    main_split::Editors,
    plugin::InstalledVoltData,
    text_input::TextInputBuilder,
    window_tab::CommonData,
};

/// Short blurb shown under each Settings section title.
fn section_blurb(kind: &str) -> &'static str {
    match kind {
        "Core" => "Theme, title bar, and general app behavior",
        "Editor" => "Fonts, wrapping, cursors, and editing helpers",
        "UI" => "Scale, sizes, and layout of panels and tabs",
        "Terminal" => "Font and line height for the integrated terminal",
        "AI" => "Assistant provider, model, and safety options",
        "Plugin Settings" => "Options exposed by installed plugins",
        _ => "Plugin-specific options",
    }
}

fn section_icon(kind: &str) -> &'static str {
    match kind {
        "Core" => LapceIcons::SETTINGS,
        "Editor" => LapceIcons::FILE,
        "UI" => LapceIcons::LAYOUT_PANEL,
        "Terminal" => LapceIcons::TERMINAL,
        "AI" => LapceIcons::AI,
        "Plugin Settings" => LapceIcons::EXTENSIONS,
        _ => LapceIcons::EXTENSIONS,
    }
}

fn friendly_field_name(field: &str) -> String {
    field.replace(['_', '-'], " ").to_title_case()
}

#[derive(Debug, Clone)]
pub enum SettingsValue {
    Float(f64),
    Integer(i64),
    String(String),
    Bool(bool),
    Dropdown(DropdownInfo),
    Empty,
}

impl From<serde_json::Value> for SettingsValue {
    fn from(v: serde_json::Value) -> Self {
        match v {
            serde_json::Value::Number(n) => {
                if n.is_f64() {
                    SettingsValue::Float(n.as_f64().unwrap())
                } else {
                    SettingsValue::Integer(n.as_i64().unwrap())
                }
            }
            serde_json::Value::String(s) => SettingsValue::String(s),
            serde_json::Value::Bool(b) => SettingsValue::Bool(b),
            _ => SettingsValue::Empty,
        }
    }
}

#[derive(Clone, Debug)]
struct SettingsItem {
    kind: String,
    name: String,
    field: String,
    description: String,
    filter_text: String,
    value: SettingsValue,
}

/// A visual group of settings rendered as a single card with row items,
/// mirroring the Cursor IDE settings layout (title, subtitle, then rows).
#[derive(Clone, Debug)]
struct SettingsSection {
    kind: String,
    title: String,
    blurb: String,
    pos: RwSignal<Point>,
    size: RwSignal<Size>,
    fields: im::Vector<SettingsItem>,
    custom: im::Vector<SettingsCustom>,
    /// Extra lowercase text matched by the search box for custom cards.
    search_text: String,
}

/// Dynamic (add/remove) list editors that are not expressible as a single
/// scalar field, rendered as extra cards inside a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsCustom {
    ExtraModels,
    McpServers,
}

/// Split a comma/newline/semicolon separated list, dropping blanks.
fn parse_list(value: &str) -> Vec<String> {
    value
        .split([',', '\n', ';'])
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect()
}

/// Persist a list of model ids as the comma separated `ai.extra-models` field.
fn persist_extra_models(models: &[(u64, String)]) {
    let joined = models
        .iter()
        .map(|(_, m)| m.trim())
        .filter(|m| !m.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    if let Ok(value) =
        serde::Serialize::serialize(&joined, toml_edit::ser::ValueSerializer::new())
    {
        LapceConfig::update_file("ai", "extra-models", value);
    }
}

impl SettingsSection {
    fn render_heading(&self, config: ReadSignal<Arc<LapceConfig>>) -> AnyView {
        let title = self.title.clone();
        let blurb = self.blurb.clone();
        let blurb_empty = blurb.is_empty();
        let pos = self.pos;
        let size = self.size;
        stack((
            label(move || title.clone()).style(move |s| {
                s.font_bold()
                    .font_size(config.get().ui.font_size() as f32 + 3.0)
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            label(move || blurb.clone()).style(move |s| {
                s.margin_top(4.0)
                    .margin_bottom(2.0)
                    .color(config.get().color(LapceColor::EDITOR_DIM))
                    .apply_if(blurb_empty, |s| s.hide())
            }),
        ))
        .on_resize(move |rect| {
            pos.set(rect.origin());
            let old_size = size.get_untracked();
            let new_size = rect.size();
            if old_size != new_size {
                size.set(new_size);
            }
        })
        .style(move |s| {
            s.flex_col()
                .width_pct(100.0)
                .padding_top(16.0)
                .padding_bottom(8.0)
                .border_bottom(1.0)
                .border_color(config.get().color(LapceColor::LAPCE_BORDER))
        })
        .into_any()
    }

    fn card_style(config: ReadSignal<Arc<LapceConfig>>) -> floem::style::Style {
        let config = config.get();
        floem::style::Style::new()
            .flex_col()
            .width_pct(100.0)
            .border(1.0)
            .border_radius(8.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .background(config.color(LapceColor::PANEL_BACKGROUND))
    }

    fn render(
        &self,
        editors: Editors,
        settings_data: SettingsData,
        config: ReadSignal<Arc<LapceConfig>>,
    ) -> AnyView {
        let fields: Vec<SettingsItem> = self.fields.iter().cloned().collect();

        let card = container(
            stack_from_iter(fields.into_iter().enumerate().map(|(i, item)| {
                let data = settings_data.clone();
                container(settings_row_view(editors, data, item))
                    .style(move |s| {
                        s.width_pct(100.0).apply_if(i > 0, |s| {
                            s.border_top(1.0).border_color(
                                config.get().color(LapceColor::LAPCE_BORDER),
                            )
                        })
                    })
                    .into_any()
            }))
            .style(|s| s.flex_col().width_pct(100.0)),
        )
        .style(move |_s| Self::card_style(config));

        let extras: Vec<AnyView> = self
            .custom
            .iter()
            .map(|custom| match custom {
                SettingsCustom::ExtraModels => {
                    extra_models_editor(editors, settings_data.clone(), config)
                }
                SettingsCustom::McpServers => {
                    mcp_servers_editor(editors, settings_data.clone(), config)
                }
            })
            .collect();

        let mut children: Vec<AnyView> =
            vec![self.render_heading(config), card.into_any()];
        children.extend(extras);

        let has_extra = !self.custom.is_empty();
        stack_from_iter(children)
            .style(move |s| {
                let s = s.flex_col().width_pct(100.0);
                if has_extra { s.row_gap(12.0) } else { s }
            })
            .into_any()
    }
}

#[derive(Clone, Debug)]
struct SettingsData {
    items: RwSignal<im::Vector<SettingsItem>>,
    kinds: RwSignal<im::Vector<(String, RwSignal<Point>)>>,
    plugin_items: RwSignal<im::Vector<SettingsItem>>,
    plugin_kinds: RwSignal<im::Vector<(String, RwSignal<Point>)>>,
    filtered_items: RwSignal<im::Vector<SettingsItem>>,
    sections: RwSignal<im::Vector<SettingsSection>>,
    common: Rc<CommonData>,
}

impl KeyPressFocus for SettingsData {
    fn get_mode(&self) -> devforge_core::mode::Mode {
        Mode::Insert
    }

    fn check_condition(
        &self,
        _condition: crate::keypress::condition::Condition,
    ) -> bool {
        false
    }

    fn run_command(
        &self,
        _command: &crate::command::LapceCommand,
        _count: Option<usize>,
        _mods: Modifiers,
    ) -> crate::command::CommandExecuted {
        CommandExecuted::No
    }

    fn receive_char(&self, _c: &str) {}
}

impl VirtualVector<SettingsItem> for SettingsData {
    fn total_len(&self) -> usize {
        self.filtered_items.get_untracked().len()
    }

    fn slice(
        &mut self,
        _range: std::ops::Range<usize>,
    ) -> impl Iterator<Item = SettingsItem> {
        Box::new(self.filtered_items.get().into_iter())
    }
}

impl SettingsData {
    pub fn new(
        cx: Scope,
        installed_plugin: RwSignal<IndexMap<VoltID, InstalledVoltData>>,
        common: Rc<CommonData>,
    ) -> Self {
        fn into_settings_map(
            data: &impl Serialize,
        ) -> serde_json::Map<String, serde_json::Value> {
            match serde_json::to_value(data).unwrap() {
                serde_json::Value::Object(h) => h,
                _ => serde_json::Map::default(),
            }
        }

        let config = common.config;
        let plugin_items = cx.create_rw_signal(im::Vector::new());
        let plugin_kinds = cx.create_rw_signal(im::Vector::new());
        let filtered_items = cx.create_rw_signal(im::Vector::new());
        let sections = cx.create_rw_signal(im::Vector::new());
        let items = cx.create_rw_signal(im::Vector::new());
        let kinds = cx.create_rw_signal(im::Vector::new());
        cx.create_effect(move |_| {
            let config = config.get();

            let mut data_items = im::Vector::new();
            let mut data_kinds = im::Vector::new();
            let mut data_sections = im::Vector::new();
            let mut item_height_accum = 0.0;
            for (kind, fields, descs, mut settings_map) in [
                (
                    "Core",
                    &CoreConfig::FIELDS[..],
                    &CoreConfig::DESCS[..],
                    into_settings_map(&config.core),
                ),
                (
                    "Editor",
                    &EditorConfig::FIELDS[..],
                    &EditorConfig::DESCS[..],
                    into_settings_map(&config.editor),
                ),
                (
                    "UI",
                    &UIConfig::FIELDS[..],
                    &UIConfig::DESCS[..],
                    into_settings_map(&config.ui),
                ),
                (
                    "Terminal",
                    &TerminalConfig::FIELDS[..],
                    &TerminalConfig::DESCS[..],
                    into_settings_map(&config.terminal),
                ),
                (
                    "AI",
                    &AiConfig::FIELDS[..],
                    &AiConfig::DESCS[..],
                    into_settings_map(&config.ai),
                ),
            ] {
                let pos = cx.create_rw_signal(Point::new(0.0, item_height_accum));
                data_kinds.push_back((kind.to_string(), pos));
                let mut section_fields = im::Vector::new();
                for (name, desc) in fields.iter().zip(descs.iter()) {
                    let field = name.replace('_', "-");

                    let value = if let Some(dropdown) =
                        config.get_dropdown_info(&kind.to_lowercase(), &field)
                    {
                        SettingsValue::Dropdown(dropdown)
                    } else {
                        let value = settings_map.remove(&field).unwrap();
                        SettingsValue::from(value)
                    };

                    let display_name = friendly_field_name(name);
                    let kind_lower = kind.to_lowercase();
                    let filter_text =
                        format!("{kind_lower} {display_name} {desc}").to_lowercase();
                    let filter_text =
                        format!("{filter_text}{}", filter_text.replace(' ', ""));
                    let item = SettingsItem {
                        kind: kind_lower,
                        name: display_name,
                        field,
                        filter_text,
                        description: desc.to_string(),
                        value,
                    };
                    section_fields.push_back(item.clone());
                    data_items.push_back(item);
                    item_height_accum += 50.0;
                }
                data_sections.push_back(SettingsSection {
                    kind: kind.to_string(),
                    title: kind.to_string(),
                    blurb: section_blurb(kind).to_string(),
                    pos,
                    size: cx.create_rw_signal(Size::ZERO),
                    fields: section_fields,
                    custom: if kind == "AI" {
                        im::Vector::from(vec![
                            SettingsCustom::ExtraModels,
                            SettingsCustom::McpServers,
                        ])
                    } else {
                        im::Vector::new()
                    },
                    search_text: if kind == "AI" {
                        "extra models mcp mcp-servers servers model context protocol"
                            .to_string()
                    } else {
                        String::new()
                    },
                });
            }

            filtered_items.set(data_items.clone());
            items.set(data_items);

            let plugins = installed_plugin.get();
            let mut setting_items = im::Vector::new();
            let mut plugin_kinds_tmp = im::Vector::new();
            let mut plugin_sections = im::Vector::new();
            for (_, volt) in plugins {
                let meta = volt.meta.get();
                let kind = meta.name.clone();
                let plugin_config = config.plugins.get(&kind);
                if let Some(config) = meta.config {
                    let pos =
                        cx.create_rw_signal(Point::new(0.0, item_height_accum));
                    plugin_kinds_tmp.push_back((meta.display_name.clone(), pos));

                    let mut section_fields = im::Vector::new();
                    let mut local_items = Vec::new();
                    for (name, config) in config {
                        let field = name.clone();

                        let display_name = friendly_field_name(&name);
                        let desc = config.description;
                        let filter_text =
                            format!("{kind} {display_name} {desc}").to_lowercase();
                        let filter_text =
                            format!("{filter_text}{}", filter_text.replace(' ', ""));

                        let value = plugin_config
                            .and_then(|config| config.get(&field).cloned())
                            .unwrap_or(config.default);
                        let value = SettingsValue::from(value);

                        let item = SettingsItem {
                            kind: kind.clone(),
                            name: display_name,
                            field,
                            filter_text,
                            description: desc.to_string(),
                            value,
                        };
                        local_items.push(item);
                        item_height_accum += 50.0;
                    }
                    local_items.sort_by_key(|i| i.name.clone());
                    section_fields.extend(local_items.iter().cloned());
                    setting_items.extend(local_items.into_iter());

                    plugin_sections.push_back(SettingsSection {
                        kind: meta.name.clone(),
                        title: meta.display_name.clone(),
                        blurb: format!(
                            "Settings from the “{}” plugin",
                            meta.display_name
                        ),
                        pos,
                        size: cx.create_rw_signal(Size::ZERO),
                        fields: section_fields,
                        custom: im::Vector::new(),
                        search_text: String::new(),
                    });
                }
            }
            plugin_items.set(setting_items);
            plugin_kinds.set(plugin_kinds_tmp);
            data_sections.extend(plugin_sections);
            sections.set(data_sections);
            kinds.set(data_kinds);
        });

        Self {
            filtered_items,
            plugin_items,
            plugin_kinds,
            items,
            kinds,
            sections,
            common,
        }
    }
}

pub fn settings_view(
    installed_plugins: RwSignal<IndexMap<VoltID, InstalledVoltData>>,
    editors: Editors,
    common: Rc<CommonData>,
) -> impl View {
    let config = common.config;

    let cx = Scope::current();
    let settings_data = SettingsData::new(cx, installed_plugins, common.clone());
    let view_settings_data = settings_data.clone();
    let plugin_kinds = settings_data.plugin_kinds;

    let search_editor = editors.make_local(cx, common.clone());
    let search_editor_id = search_editor.id();
    let doc = search_editor.doc_signal();

    let items = settings_data.items;
    let kinds = settings_data.kinds;
    let filtered_items_signal = settings_data.filtered_items;
    let sections_signal = settings_data.sections;
    let rendered_sections = create_rw_signal(im::Vector::new());
    let match_count = create_rw_signal(0usize);
    let search_query = create_rw_signal(String::new());
    create_effect(move |_| {
        let doc = doc.get();
        let pattern = doc.buffer.with(|b| b.to_string().to_lowercase());
        search_query.set(pattern.clone());
        let plugin_items = settings_data.plugin_items.get();
        let mut items = items.get();

        // Match each query word independently so "font AI" matches fields that
        // mention either word, mirroring Cursor's settings search behavior.
        let queries: im::Vector<String> = pattern
            .split_whitespace()
            .filter(|q| !q.is_empty())
            .map(|q| q.to_string())
            .collect();

        if pattern.is_empty() {
            items.extend(plugin_items);
            match_count.set(items.len());
            filtered_items_signal.set(items);
            rendered_sections.set(sections_signal.get_untracked());
            return;
        }

        let mut matched: im::Vector<SettingsItem> = im::Vector::new();
        for item in items.iter().chain(plugin_items.iter()) {
            let hay = &item.filter_text;
            if queries.iter().all(|q| hay.contains(q.as_str())) {
                matched.push_back(item.clone());
            }
        }

        let mut new_sections = im::Vector::new();
        let mut custom_matches = 0usize;
        for mut section in sections_signal.get_untracked() {
            section.fields = section
                .fields
                .into_iter()
                .filter(|item| {
                    let hay = &item.filter_text;
                    queries.iter().all(|q| hay.contains(q.as_str()))
                })
                .collect();
            let custom_match = !section.custom.is_empty()
                && !section.search_text.is_empty()
                && queries
                    .iter()
                    .all(|q| section.search_text.contains(q.as_str()));
            if custom_match {
                custom_matches += 1;
                section.fields = section
                    .fields
                    .into_iter()
                    .chain(
                        items
                            .iter()
                            .filter(|i| i.kind == section.kind.to_lowercase())
                            .cloned(),
                    )
                    .collect();
            }
            if !section.fields.is_empty() || custom_match {
                new_sections.push_back(section);
            }
        }
        match_count.set(matched.len() + custom_matches);
        filtered_items_signal.set(matched);
        rendered_sections.set(new_sections);
    });

    let ensure_visible = create_rw_signal(Rect::ZERO);
    let settings_content_size = create_rw_signal(Size::ZERO);
    let scroll_pos = create_rw_signal(Point::ZERO);

    let current_kind = {
        create_memo(move |_| {
            let scroll_pos = scroll_pos.get();
            let scroll_y = scroll_pos.y + 30.0;

            let plugin_kinds = plugin_kinds.get_untracked();
            for (kind, pos) in plugin_kinds.iter().rev() {
                if pos.get_untracked().y < scroll_y {
                    return kind.to_string();
                }
            }

            let kinds = kinds.get();
            for (kind, pos) in kinds.iter().rev() {
                if pos.get_untracked().y < scroll_y {
                    return kind.to_string();
                }
            }

            kinds.get(0).unwrap().0.to_string()
        })
    };

    let switcher_item = move |k: String,
                              pos: Box<dyn Fn() -> Option<RwSignal<Point>>>,
                              margin: f32| {
        let kind = k.clone();
        let icon = section_icon(&k);
        let blurb = section_blurb(&k).to_string();
        let show_blurb = margin < 1.0 && !blurb.is_empty() && k != "Plugin Settings";
        container(
            stack((
                svg(move || config.get().ui_svg(icon)).style(move |s| {
                    let size = config.get().ui.icon_size() as f32;
                    s.size(size, size)
                        .color(config.get().color(LapceColor::LAPCE_ICON_ACTIVE))
                        .margin_right(8.0)
                        .apply_if(margin > 0.0, |s| s.hide())
                }),
                stack((
                    label(move || k.clone()).style(move |s| {
                        s.font_bold()
                            .text_ellipsis()
                            .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                    }),
                    label(move || blurb.clone()).style(move |s| {
                        s.font_size(
                            (config.get().ui.font_size() as f32 - 1.0).max(11.0),
                        )
                        .margin_top(1.0)
                        .text_ellipsis()
                        .color(config.get().color(LapceColor::EDITOR_DIM))
                        .apply_if(!show_blurb, |s| s.hide())
                    }),
                ))
                .style(|s| s.flex_col().min_width(0.0).flex_grow(1.0)),
            ))
            .style(move |s| {
                s.items_center()
                    .width_pct(100.0)
                    .padding_left(margin)
                    .padding_vert(6.0)
            }),
        )
        .on_click_stop(move |_| {
            if let Some(pos) = pos() {
                ensure_visible.set(
                    settings_content_size
                        .get_untracked()
                        .to_rect()
                        .with_origin(pos.get_untracked()),
                );
            }
        })
        .style(move |s| {
            let config = config.get();
            s.padding_horiz(12.0)
                .width_pct(100.0)
                .border_radius(6.0)
                .apply_if(kind == current_kind.get(), |s| {
                    s.background(config.color(LapceColor::PANEL_CURRENT_BACKGROUND))
                })
                .hover(|s| {
                    s.cursor(CursorStyle::Pointer).background(
                        config.color(LapceColor::PANEL_HOVERED_BACKGROUND),
                    )
                })
                .active(|s| {
                    s.background(
                        config.color(LapceColor::PANEL_HOVERED_ACTIVE_BACKGROUND),
                    )
                })
        })
    };

    let switcher = || {
        stack((
            label(|| "CATEGORIES".to_string()).style(move |s| {
                s.padding_horiz(12.0)
                    .padding_bottom(8.0)
                    .font_size((config.get().ui.font_size() as f32 - 1.0).max(10.0))
                    .font_bold()
                    .color(config.get().color(LapceColor::EDITOR_DIM))
            }),
            dyn_stack(
                move || kinds.get().clone(),
                |(k, _)| k.clone(),
                move |(k, pos)| switcher_item(k, Box::new(move || Some(pos)), 0.0),
            )
            .style(|s| s.flex_col().width_pct(100.0).row_gap(2.0)),
            stack((
                switcher_item(
                    "Plugin Settings".to_string(),
                    Box::new(move || {
                        plugin_kinds
                            .with_untracked(|k| k.get(0).map(|(_, pos)| *pos))
                    }),
                    0.0,
                ),
                dyn_stack(
                    move || plugin_kinds.get(),
                    |(k, _)| k.clone(),
                    move |(k, pos)| {
                        switcher_item(k, Box::new(move || Some(pos)), 10.0)
                    },
                )
                .style(|s| s.flex_col().width_pct(100.0).row_gap(2.0)),
            ))
            .style(move |s| {
                s.width_pct(100.0)
                    .flex_col()
                    .margin_top(10.0)
                    .row_gap(2.0)
                    .apply_if(plugin_kinds.with(|k| k.is_empty()), |s| s.hide())
            }),
        ))
        .style(move |s| {
            s.width_pct(100.0)
                .flex_col()
                .font_size(config.get().ui.font_size() as f32)
        })
    };

    let workbench_command = common.workbench_command;
    let settings_header = {
        let workbench_command = workbench_command;
        stack((
            stack((
                label(|| "Settings".to_string()).style(move |s| {
                    s.font_bold()
                        .font_size(config.get().ui.font_size() as f32 + 6.0)
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),
                label(|| "Changes save automatically to settings.toml".to_string())
                    .style(move |s| {
                        s.margin_top(4.0)
                            .color(config.get().color(LapceColor::EDITOR_DIM))
                    }),
            ))
            .style(|s| s.flex_col().items_start().flex_grow(1.0).min_width(0.0)),
            stack((
                settings_quick_action(config, LapceIcons::FILE, "Open File", {
                    let workbench_command = workbench_command;
                    move || {
                        workbench_command
                            .send(LapceWorkbenchCommand::OpenSettingsFile);
                    }
                }),
                settings_quick_action(
                    config,
                    LapceIcons::SYMBOL_COLOR,
                    "Theme Colors",
                    {
                        let workbench_command = workbench_command;
                        move || {
                            workbench_command
                                .send(LapceWorkbenchCommand::OpenThemeColorSettings);
                        }
                    },
                ),
                settings_quick_action(config, LapceIcons::KEYBOARD, "Keyboard", {
                    let workbench_command = workbench_command;
                    move || {
                        workbench_command
                            .send(LapceWorkbenchCommand::OpenKeyboardShortcuts);
                    }
                }),
            ))
            .style(|s| s.items_center().col_gap(8.0)),
        ))
        .style(|s| {
            s.width_pct(100.0)
                .items_center()
                .padding_horiz(28.0)
                .padding_top(20.0)
                .padding_bottom(8.0)
        })
    };

    stack((
        container({
            scroll({
                container(switcher()).style(|s| {
                    s.padding_vert(16.0).padding_horiz(8.0).width_pct(100.0)
                })
            })
            .style(|s| s.absolute().size_pct(100.0, 100.0))
        })
        .style(move |s| {
            s.height_pct(100.0)
                .width(240.0)
                .border_right(1.0)
                .border_color(config.get().color(LapceColor::LAPCE_BORDER))
                .background(config.get().color(LapceColor::PANEL_BACKGROUND))
        }),
        stack((
            settings_header,
            container({
                TextInputBuilder::new()
                    .build_editor(search_editor)
                    .placeholder(|| {
                        "Search settings (theme, font, AI, terminal…)".to_string()
                    })
                    .keyboard_navigable()
                    .style(move |s| {
                        s.width_pct(100.0)
                            .border_radius(6.0)
                            .border(1.0)
                            .border_color(
                                config.get().color(LapceColor::LAPCE_BORDER),
                            )
                    })
                    .request_focus(|| {})
            })
            .style(|s| s.padding_horiz(28.0).padding_bottom(12.0).padding_top(4.0)),
            container({
                stack((
                    scroll({
                        dyn_stack(
                            move || rendered_sections.get(),
                            |section| section.kind.clone(),
                            move |section| {
                                section.render(
                                    editors,
                                    view_settings_data.clone(),
                                    config,
                                )
                            },
                        )
                        .style(|s| {
                            s.flex_col()
                                .padding_horiz(28.0)
                                .padding_bottom(40.0)
                                .min_width_pct(100.0)
                                .max_width(720.0)
                                .row_gap(4.0)
                        })
                    })
                    .on_scroll(move |rect| {
                        scroll_pos.set(rect.origin());
                    })
                    .ensure_visible(move || ensure_visible.get())
                    .on_resize(move |rect| {
                        settings_content_size.set(rect.size());
                    })
                    .style(|s| s.absolute().size_pct(100.0, 100.0)),
                    label(move || {
                        let q = search_query.get();
                        if q.is_empty() || match_count.get() > 0 {
                            String::new()
                        } else {
                            format!("No settings match “{q}”")
                        }
                    })
                    .style(move |s| {
                        let empty =
                            !search_query.get().is_empty() && match_count.get() == 0;
                        s.absolute()
                            .margin_top(40.0)
                            .margin_left(28.0)
                            .color(config.get().color(LapceColor::EDITOR_DIM))
                            .apply_if(!empty, |s| s.hide())
                    }),
                ))
                .style(|s| s.size_pct(100.0, 100.0))
            })
            .style(|s| s.size_pct(100.0, 100.0)),
        ))
        .style(|s| s.flex_col().size_pct(100.0, 100.0)),
    ))
    .style(|s| s.absolute().size_pct(100.0, 100.0))
    .on_cleanup(move || {
        // Floem tab children dispose their scope on close; unregister so
        // remove_editor does not later hit a disposed doc signal.
        editors.remove(search_editor_id);
    })
    .debug_name("Settings")
}

fn settings_quick_action(
    config: ReadSignal<Arc<LapceConfig>>,
    icon: &'static str,
    title: &'static str,
    on_click: impl Fn() + 'static,
) -> impl View {
    stack((
        svg(move || config.get().ui_svg(icon)).style(move |s| {
            let size = config.get().ui.icon_size() as f32;
            s.size(size, size)
                .color(config.get().color(LapceColor::LAPCE_ICON_ACTIVE))
        }),
        label(move || title.to_string()).style(move |s| {
            s.color(config.get().color(LapceColor::EDITOR_FOREGROUND))
        }),
    ))
    .style(move |s| {
        let config = config.get();
        s.items_center()
            .col_gap(6.0)
            .padding_horiz(10.0)
            .padding_vert(6.0)
            .border_radius(6.0)
            .border(1.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .cursor(CursorStyle::Pointer)
            .hover(|s| {
                s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
            })
    })
    .on_click_stop(move |_| on_click())
}

/// Cursor-style settings row: label + description on the left, the control on
/// the right, separated by a subtle divider between consecutive rows.
fn settings_row_view(
    editors: Editors,
    settings_data: SettingsData,
    item: SettingsItem,
) -> impl View + use<> {
    let config = settings_data.common.config;

    let is_ticked = if let SettingsValue::Bool(is_ticked) = &item.value {
        Some(*is_ticked)
    } else {
        None
    };

    let timer = create_rw_signal(TimerToken::INVALID);

    let editor_value = match &item.value {
        SettingsValue::Float(n) => Some(n.to_string()),
        SettingsValue::Integer(n) => Some(n.to_string()),
        SettingsValue::String(s) => Some(s.to_string()),
        SettingsValue::Bool(_) => None,
        SettingsValue::Dropdown(_) => None,
        SettingsValue::Empty => None,
    };

    let view = {
        let item = item.clone();
        move || {
            let cx = Scope::current();
            if let Some(editor_value) = editor_value {
                let text_input_view = TextInputBuilder::new()
                    .value(editor_value)
                    .build(cx, editors, settings_data.common);

                let doc = text_input_view.doc_signal();

                let kind = item.kind.clone();
                let field = item.field.clone();
                let item_value = item.value.clone();
                create_effect(move |last| {
                    let doc = doc.get_untracked();
                    let rev = doc.buffer.with(|b| b.rev());
                    if last.is_none() {
                        return rev;
                    }
                    if last == Some(rev) {
                        return rev;
                    }
                    let kind = kind.clone();
                    let field = field.clone();
                    let buffer = doc.buffer;
                    let item_value = item_value.clone();
                    let token =
                        exec_after(Duration::from_millis(500), move |token| {
                            let Some(timer) = timer.try_get_untracked() else {
                                return;
                            };
                            if timer != token {
                                return;
                            }

                            let value = buffer.with_untracked(|b| b.to_string());
                            let value = value.trim();
                            let value = match &item_value {
                                SettingsValue::Float(_) => {
                                    value.parse::<f64>().ok().and_then(|v| {
                                        serde::Serialize::serialize(
                                            &v,
                                            toml_edit::ser::ValueSerializer::new(),
                                        )
                                        .ok()
                                    })
                                }
                                SettingsValue::Integer(_) => {
                                    value.parse::<i64>().ok().and_then(|v| {
                                        serde::Serialize::serialize(
                                            &v,
                                            toml_edit::ser::ValueSerializer::new(),
                                        )
                                        .ok()
                                    })
                                }
                                _ => serde::Serialize::serialize(
                                    &value,
                                    toml_edit::ser::ValueSerializer::new(),
                                )
                                .ok(),
                            };

                            if let Some(value) = value {
                                LapceConfig::update_file(&kind, &field, value);
                            }
                        });
                    timer.set(token);

                    rev
                });

                text_input_view
                    .keyboard_navigable()
                    .style(move |s| {
                        s.width(220.0)
                            .border(1.0)
                            .border_radius(6.0)
                            .border_color(
                                config.get().color(LapceColor::LAPCE_BORDER),
                            )
                            .focus(|s| {
                                s.border_color(
                                    config.get().color(LapceColor::EDITOR_FOCUS),
                                )
                            })
                    })
                    .into_any()
            } else if let SettingsValue::Dropdown(dropdown) = &item.value {
                let expanded = create_rw_signal(false);
                let current_value = dropdown
                    .items
                    .get(dropdown.active_index)
                    .or_else(|| dropdown.items.last())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let current_value = create_rw_signal(current_value);

                dropdown_view(
                    &item,
                    current_value,
                    dropdown,
                    expanded,
                    settings_data.common.window_common.size,
                    config,
                )
                .into_any()
            } else {
                empty().into_any()
            }
        }
    };

    let bool_state: Option<RwSignal<bool>> = is_ticked.map(create_rw_signal);
    let bool_toggle = if let Some(checked) = bool_state {
        let kind = item.kind.clone();
        let field = item.field.clone();
        create_effect(move |last| {
            let checked = checked.get();
            if last.is_none() {
                return;
            }
            if let Ok(value) = serde::Serialize::serialize(
                &checked,
                toml_edit::ser::ValueSerializer::new(),
            ) {
                LapceConfig::update_file(&kind, &field, value);
            }
        });

        container(toggle_switch(move || checked.get(), config))
            .style(|s| s.items_center())
            .into_any()
    } else {
        empty().into_any()
    };

    let has_bool = is_ticked.is_some();

    stack((
        stack((
            label(move || item.name.clone()).style(move |s| {
                s.font_bold()
                    .text_ellipsis()
                    .min_width(0.0)
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            label(move || item.description.clone()).style(move |s| {
                s.margin_top(3.0)
                    .min_width(0.0)
                    .line_height(1.4)
                    .font_size((config.get().ui.font_size() as f32 - 1.0).max(11.0))
                    .color(config.get().color(LapceColor::EDITOR_DIM))
            }),
        ))
        .style(|s| s.flex_col().min_width(0.0).flex_grow(1.0).flex_basis(0.0)),
        container(bool_toggle).style(move |s| s.apply_if(!has_bool, |s| s.hide())),
        container(view())
            .style(move |s| s.apply_if(has_bool, |s| s.hide()).flex_shrink(0.0)),
    ))
    .on_click_stop(move |_| {
        if let Some(checked) = bool_state {
            checked.update(|checked| {
                *checked = !*checked;
            });
        }
    })
    .style(move |s| {
        s.width_pct(100.0)
            .items_center()
            .col_gap(16.0)
            .padding_horiz(14.0)
            .padding_vert(12.0)
            .apply_if(has_bool, |s| s.cursor(CursorStyle::Pointer))
            .hover(|s| {
                s.background(
                    config.get().color(LapceColor::PANEL_HOVERED_BACKGROUND),
                )
            })
    })
    .into_any()
}

pub fn checkbox(
    checked: impl Fn() -> bool + 'static,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    const CHECKBOX_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="-2 -2 16 16"><polygon points="5.19,11.83 0.18,7.44 1.82,5.56 4.81,8.17 10,1.25 12,2.75" /></svg>"#;
    let svg_str = move || if checked() { CHECKBOX_SVG } else { "" }.to_string();

    svg(svg_str).style(move |s| {
        let config = config.get();
        let size = config.ui.font_size() as f32;
        let color = config.color(LapceColor::EDITOR_FOREGROUND);

        s.min_width(size)
            .size(size, size)
            .color(color)
            .border_color(color)
            .border(1.)
            .border_radius(2.)
    })
}

/// Animated switch, mirroring the Cursor IDE settings toggles. The knob slides
/// with a short eased transition when the value flips.
pub fn toggle_switch(
    checked: impl Fn() -> bool + 'static,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View {
    const KNOB: f64 = 14.0;
    const INSET: f64 = 2.0;
    const TRACK_W: f64 = 34.0;
    const TRACK_H: f64 = 18.0;

    let checked = Rc::new(checked);

    container(empty().style({
        let checked = checked.clone();
        move |s| {
            let config = config.get();
            s.size(KNOB, KNOB)
                .border_radius(KNOB / 2.0)
                .background(config.color(LapceColor::EDITOR_BACKGROUND))
                .box_shadow_blur(1.0)
                .box_shadow_color(config.color(LapceColor::LAPCE_DROPDOWN_SHADOW))
                .margin_left(if checked() {
                    TRACK_W - KNOB - INSET
                } else {
                    INSET
                })
                .transition(
                    MarginLeft,
                    Transition::ease_in_out(StdDuration::from_millis(140)),
                )
        }
    }))
    .style(move |s| {
        let config = config.get();
        s.size(TRACK_W, TRACK_H)
            .items_center()
            .border_radius(TRACK_H / 2.0)
            .background(if checked() {
                config.color(LapceColor::EDITOR_FOCUS)
            } else {
                config.color(LapceColor::LAPCE_BORDER)
            })
            .transition_background(Transition::ease_in_out(
                StdDuration::from_millis(140),
            ))
    })
    .style(|s| s.cursor(CursorStyle::Pointer))
}

struct BTreeMapVirtualList(BTreeMap<String, String>);

impl VirtualVector<(String, String)> for BTreeMapVirtualList {
    fn total_len(&self) -> usize {
        self.0.len()
    }

    fn slice(
        &mut self,
        range: std::ops::Range<usize>,
    ) -> impl Iterator<Item = (String, String)> {
        Box::new(
            self.0
                .iter()
                .enumerate()
                .filter_map(|(index, (k, v))| {
                    if range.contains(&index) {
                        Some((k.to_string(), v.to_string()))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .into_iter(),
        )
    }
}

fn color_section_list(
    kind: &str,
    header: &str,
    list: impl Fn() -> BTreeMap<String, String> + 'static,
    max_width: Memo<f64>,
    text_height: Memo<f64>,
    editors: Editors,
    common: Rc<CommonData>,
) -> impl View {
    let config = common.config;

    let kind = kind.to_string();
    stack((
        text(header).style(|s| {
            s.margin_top(10)
                .margin_horiz(20)
                .font_bold()
                .line_height(2.0)
        }),
        virtual_stack(
            move || BTreeMapVirtualList(list()),
            move |(key, _)| key.to_owned(),
            move |(key, value)| {
                let cx = Scope::current();
                let text_input_view = TextInputBuilder::new()
                    .value(value.clone())
                    .build(cx, editors, common.clone());
                let doc = text_input_view.doc_signal();

                {
                    let kind = kind.clone();
                    let key = key.clone();
                    let doc = text_input_view.doc_signal();
                    create_effect(move |_| {
                        let doc = doc.get_untracked();
                        let config = config.get();
                        let current = doc.buffer.with_untracked(|b| b.to_string());

                        let value = match kind.as_str() {
                            "base" => config.color_theme.base.get(&key),
                            "ui" => config.color_theme.ui.get(&key),
                            "syntax" => config.color_theme.syntax.get(&key),
                            _ => None,
                        };

                        if let Some(value) = value {
                            if value != &current {
                                doc.reload(Rope::from(value.to_string()), true);
                            }
                        }
                    });
                }

                {
                    let timer = create_rw_signal(TimerToken::INVALID);
                    let kind = kind.clone();
                    let field = key.clone();
                    create_effect(move |last| {
                        let doc = doc.get_untracked();

                        let rev = doc.buffer.with(|b| b.rev());
                        if last.is_none() {
                            return rev;
                        }
                        if last == Some(rev) {
                            return rev;
                        }
                        let kind = kind.clone();
                        let field = field.clone();
                        let buffer = doc.buffer;
                        let token =
                            exec_after(Duration::from_millis(500), move |token| {
                                if let Some(timer) = timer.try_get_untracked() {
                                    if timer == token {
                                        let value =
                                            buffer.with_untracked(|b| b.to_string());

                                        let config = config.get_untracked();
                                        let default = match kind.as_str() {
                                            "base" => config
                                                .default_color_theme()
                                                .base
                                                .get(&field),
                                            "ui" => config
                                                .default_color_theme()
                                                .ui
                                                .get(&field),
                                            "syntax" => config
                                                .default_color_theme()
                                                .syntax
                                                .get(&field),
                                            _ => None,
                                        };

                                        if default != Some(&value) {
                                            let value = serde::Serialize::serialize(
                                                &value,
                                                toml_edit::ser::ValueSerializer::new(
                                                ),
                                            )
                                            .ok();

                                            if let Some(value) = value {
                                                LapceConfig::update_file(
                                                    &format!("color-theme.{kind}"),
                                                    &field,
                                                    value,
                                                );
                                            }
                                        } else {
                                            LapceConfig::reset_setting(
                                                &format!("color-theme.{kind}"),
                                                &field,
                                            );
                                        }
                                    }
                                }
                            });
                        timer.set(token);

                        rev
                    });
                }

                let local_kind = kind.clone();
                let local_key = key.clone();
                stack((
                    text(&key).style(move |s| {
                        s.width(max_width.get()).margin_left(20).margin_right(10)
                    }),
                    text_input_view.keyboard_navigable().style(move |s| {
                        s.width(150.0)
                            .margin_vert(6)
                            .border(1)
                            .border_radius(6)
                            .border_color(
                                config.get().color(LapceColor::LAPCE_BORDER),
                            )
                    }),
                    empty().style(move |s| {
                        let size = text_height.get() + 12.0;
                        let config = config.get();
                        let color = match local_kind.as_str() {
                            "base" => config.color.base.get(&local_key),
                            "ui" => config.color.ui.get(&local_key).copied(),
                            "syntax" => config.color.syntax.get(&local_key).copied(),
                            _ => None,
                        };
                        s.border(1)
                            .border_radius(6)
                            .size(size, size)
                            .margin_left(10)
                            .border_color(config.color(LapceColor::LAPCE_BORDER))
                            .background(color.unwrap_or_else(|| {
                                config.color(LapceColor::EDITOR_FOREGROUND)
                            }))
                    }),
                    {
                        let kind = kind.clone();
                        let key = key.clone();
                        let local_key = key.clone();
                        let local_kind = kind.clone();
                        text("Reset")
                            .on_click_stop(move |_| {
                                LapceConfig::reset_setting(
                                    &format!("color-theme.{local_kind}"),
                                    &local_key,
                                );
                            })
                            .style(move |s| {
                                let doc = doc.get_untracked();
                                let config = config.get();
                                let buffer = doc.buffer;
                                let content = buffer.with(|b| b.to_string());

                                let same = match kind.as_str() {
                                    "base" => {
                                        config.default_color_theme().base.get(&key)
                                            == Some(&content)
                                    }
                                    "ui" => {
                                        config.default_color_theme().ui.get(&key)
                                            == Some(&content)
                                    }
                                    "syntax" => {
                                        config.default_color_theme().syntax.get(&key)
                                            == Some(&content)
                                    }
                                    _ => false,
                                };

                                s.margin_left(10)
                                    .padding(6)
                                    .cursor(CursorStyle::Pointer)
                                    .border(1)
                                    .border_radius(6)
                                    .border_color(
                                        config.color(LapceColor::LAPCE_BORDER),
                                    )
                                    .apply_if(same, |s| s.hide())
                                    .active(|s| {
                                        s.background(
                                            config
                                                .color(LapceColor::PANEL_BACKGROUND),
                                        )
                                    })
                            })
                    },
                ))
                .style(|s| s.items_center())
            },
        )
        .item_size_fixed(move || text_height.get() + 24.0)
        .style(|s| s.flex_col().padding_right(20)),
    ))
    .style(|s| s.flex_col())
}

pub fn theme_color_settings_view(
    editors: Editors,
    common: Rc<CommonData>,
) -> impl View {
    let config = common.config;

    let text_height = create_memo(move |_| {
        let mut text_layout = TextLayout::new();
        let config = config.get();
        let family: Vec<FamilyOwned> =
            FamilyOwned::parse_list(&config.ui.font_family).collect();
        let attrs = Attrs::new()
            .family(&family)
            .font_size(config.ui.font_size() as f32);
        let attrs_list = AttrsList::new(attrs);
        text_layout.set_text("W", attrs_list, None);
        text_layout.size().height
    });

    let max_width = create_memo(move |_| {
        let mut text_layout = TextLayout::new();
        let config = config.get();
        let family: Vec<FamilyOwned> =
            FamilyOwned::parse_list(&config.ui.font_family).collect();
        let attrs = Attrs::new()
            .family(&family)
            .font_size(config.ui.font_size() as f32);
        let attrs_list = AttrsList::new(attrs);

        let mut max_width = 0.0;
        for key in config.color_theme.ui.keys() {
            text_layout.set_text(key, attrs_list.clone(), None);
            let width = text_layout.size().width;
            if width > max_width {
                max_width = width;
            }
        }
        for key in config.color_theme.syntax.keys() {
            text_layout.set_text(key, attrs_list.clone(), None);
            let width = text_layout.size().width;
            if width > max_width {
                max_width = width;
            }
        }
        max_width
    });

    let cx = Scope::current();
    let search_editor = editors.make_local(cx, common.clone());
    let search_editor_id = search_editor.id();
    let buffer = search_editor.doc_signal().get_untracked().buffer;

    scroll(
        stack((
            container({
                TextInputBuilder::new()
                    .build_editor(search_editor)
                    .placeholder(|| "Search Settings".to_string())
                    .keyboard_navigable()
                    .style(move |s| {
                        s.width_pct(100.0)
                            .border_radius(6.0)
                            .border(1.0)
                            .border_color(
                                config.get().color(LapceColor::LAPCE_BORDER),
                            )
                    })
                    .request_focus(|| {})
            })
            .style(|s| s.padding_vert(20.0).padding_horiz(20.0)),
            color_section_list(
                "base",
                "Base Colors",
                move || {
                    let filter = buffer.get().text().to_string();
                    config.with(|c| {
                        c.color_theme
                            .base
                            .0
                            .iter()
                            .filter_map(|x| {
                                if x.0.contains(&filter) {
                                    Some((x.0.clone(), x.1.clone()))
                                } else {
                                    None
                                }
                            })
                            .collect::<BTreeMap<String, String>>()
                    })
                },
                max_width,
                text_height,
                editors,
                common.clone(),
            ),
            color_section_list(
                "syntax",
                "Syntax Colors",
                move || {
                    let filter = buffer.get().text().to_string();
                    config.with(|c| {
                        c.color_theme
                            .syntax
                            .iter()
                            .filter_map(|x| {
                                if x.0.contains(&filter) {
                                    Some((x.0.clone(), x.1.clone()))
                                } else {
                                    None
                                }
                            })
                            .collect::<BTreeMap<String, String>>()
                    })
                },
                max_width,
                text_height,
                editors,
                common.clone(),
            ),
            color_section_list(
                "ui",
                "UI Colors",
                move || {
                    let filter = buffer.get().text().to_string();
                    config.with(|c| {
                        c.color_theme
                            .ui
                            .iter()
                            .filter_map(|x| {
                                if x.0.contains(&filter) {
                                    Some((x.0.clone(), x.1.clone()))
                                } else {
                                    None
                                }
                            })
                            .collect::<BTreeMap<String, String>>()
                    })
                },
                max_width,
                text_height,
                editors,
                common.clone(),
            ),
        ))
        .style(|s| s.flex_col()),
    )
    .style(|s| s.absolute().size_full())
    .on_cleanup(move || {
        editors.remove(search_editor_id);
    })
    .debug_name("Theme Color Settings")
}

/// A titled card wrapper used by the dynamic (add/remove) list editors.
fn build_card(
    title: String,
    subtitle: String,
    body: AnyView,
    config: ReadSignal<Arc<LapceConfig>>,
) -> AnyView {
    let subtitle_empty = subtitle.is_empty();
    stack((
        stack((
            label(move || title.clone()).style(move |s| {
                s.font_bold()
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            label(move || subtitle.clone()).style(move |s| {
                s.margin_top(2.0)
                    .font_size((config.get().ui.font_size() as f32 - 1.0).max(11.0))
                    .color(config.get().color(LapceColor::EDITOR_DIM))
                    .apply_if(subtitle_empty, |s| s.hide())
            }),
        ))
        .style(|s| s.flex_col().padding_horiz(14.0).padding_vert(12.0)),
        body,
    ))
    .style(move |s| {
        s.flex_col()
            .width_pct(100.0)
            .border(1.0)
            .border_radius(8.0)
            .border_color(config.get().color(LapceColor::LAPCE_BORDER))
            .background(config.get().color(LapceColor::PANEL_BACKGROUND))
    })
    .into_any()
}

fn icon_button(
    icon: &'static str,
    title: &'static str,
    on_click: impl Fn() + 'static,
    config: ReadSignal<Arc<LapceConfig>>,
) -> AnyView {
    container(svg(move || config.get().ui_svg(icon)).style(move |s| {
        let size = config.get().ui.icon_size() as f32;
        s.size(size, size)
            .color(config.get().color(LapceColor::LAPCE_ICON_ACTIVE))
    }))
    .on_click_stop(move |_| on_click())
    .style(move |s| {
        let config = config.get();
        s.padding(5.0)
            .border_radius(6.0)
            .cursor(CursorStyle::Pointer)
            .hover(|s| {
                s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
            })
    })
    .debug_name(title)
    .into_any()
}

/// Dynamic editor for the `ai.extra-models` comma separated list. Each row is a
/// text input plus a remove button; new rows append on demand.
fn extra_models_editor(
    editors: Editors,
    settings_data: SettingsData,
    config: ReadSignal<Arc<LapceConfig>>,
) -> AnyView {
    let common = settings_data.common.clone();
    let models: RwSignal<Vec<(u64, String)>> = create_rw_signal(
        parse_list(&common.config.get_untracked().ai.extra_models)
            .into_iter()
            .enumerate()
            .map(|(i, m)| (i as u64, m))
            .collect(),
    );
    let next_id = create_rw_signal(models.with_untracked(|m| m.len() as u64));

    let rows = dyn_stack(
        move || models.get(),
        |(id, _)| *id,
        move |(id, value)| {
            let cx = Scope::current();
            let input = TextInputBuilder::new().value(value.clone()).build(
                cx,
                editors,
                common.clone(),
            );
            let doc = input.doc_signal();
            create_effect(move |last| {
                let doc = doc.get_untracked();
                let rev = doc.buffer.with(|b| b.rev());
                if last == Some(rev) || last.is_none() {
                    return rev;
                }
                let text = doc.buffer.with_untracked(|b| b.to_string());
                models.update(|models| {
                    if let Some(entry) =
                        models.iter_mut().find(|(rid, _)| *rid == id)
                    {
                        entry.1 = text;
                    }
                });
                let snapshot = models.get_untracked();
                persist_extra_models(&snapshot);
                rev
            });

            stack((
                input.keyboard_navigable().style(move |s| {
                    s.width(260.0)
                        .border(1.0)
                        .border_radius(6.0)
                        .border_color(config.get().color(LapceColor::LAPCE_BORDER))
                        .focus(|s| {
                            s.border_color(
                                config.get().color(LapceColor::EDITOR_FOCUS),
                            )
                        })
                }),
                icon_button(
                    LapceIcons::CLOSE,
                    "Remove",
                    move || {
                        models.update(|models| models.retain(|(rid, _)| *rid != id));
                        models.with_untracked(|m| persist_extra_models(m));
                    },
                    config,
                ),
            ))
            .style(|s| {
                s.items_center()
                    .col_gap(8.0)
                    .padding_horiz(14.0)
                    .padding_vert(6.0)
            })
        },
    )
    .style(|s| s.flex_col().width_pct(100.0));

    let add =
        container(label(|| "+ Add model".to_string()).style(move |s| {
            s.color(config.get().color(LapceColor::EDITOR_FOREGROUND))
        }))
        .on_click_stop(move |_| {
            let id = next_id.get_untracked();
            next_id.set(id + 1);
            models.update(|models| models.push((id, String::new())));
        })
        .style(move |s| {
            let config = config.get();
            s.padding_horiz(14.0)
                .padding_vert(10.0)
                .border_top(1.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .cursor(CursorStyle::Pointer)
                .hover(|s| {
                    s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
                })
        });

    let empty_hint = label(|| "No extra models yet".to_string()).style(move |s| {
        s.padding_horiz(14.0)
            .padding_vert(8.0)
            .font_size((config.get().ui.font_size() as f32 - 1.0).max(11.0))
            .color(config.get().color(LapceColor::EDITOR_DIM))
            .apply_if(!models.with(|m| m.is_empty()), |s| s.hide())
    });

    let body = stack((rows, empty_hint, add))
        .style(|s| s.flex_col().width_pct(100.0))
        .into_any();

    build_card(
        "Extra Models".to_string(),
        "Additional model ids available in the picker (one per row)".to_string(),
        body,
        config,
    )
}

/// Dynamic editor for the `ai.mcp-servers` list. Mirrors Cursor's MCP server
/// table: name, command, args, and an enabled toggle, with add/remove.
fn mcp_servers_editor(
    editors: Editors,
    settings_data: SettingsData,
    config: ReadSignal<Arc<LapceConfig>>,
) -> AnyView {
    let common = settings_data.common.clone();
    let servers: RwSignal<Vec<(u64, McpServerConfig)>> = create_rw_signal(
        common
            .config
            .get_untracked()
            .ai
            .mcp_servers
            .iter()
            .cloned()
            .enumerate()
            .map(|(i, s)| (i as u64, s))
            .collect(),
    );
    let next_id = create_rw_signal(servers.with_untracked(|s| s.len() as u64));

    let rows = dyn_stack(
        move || servers.get(),
        |(id, _)| *id,
        move |(id, server)| {
            let cx = Scope::current();
            let name_input = TextInputBuilder::new()
                .value(server.name.clone())
                .build(cx, editors, common.clone());
            let name_doc = name_input.doc_signal();
            create_effect(move |last| {
                let doc = name_doc.get_untracked();
                let rev = doc.buffer.with(|b| b.rev());
                if last.is_none() || last == Some(rev) {
                    return rev;
                }
                let text = doc.buffer.with_untracked(|b| b.to_string());
                servers.update(|servers| {
                    if let Some(entry) =
                        servers.iter_mut().find(|(sid, _)| *sid == id)
                    {
                        entry.1.name = text;
                    }
                });
                let snapshot = servers.get_untracked();
                persist_mcp_servers(&snapshot);
                rev
            });

            let command_input = TextInputBuilder::new()
                .value(server.command.clone())
                .build(cx, editors, common.clone());
            let command_doc = command_input.doc_signal();
            create_effect(move |last| {
                let doc = command_doc.get_untracked();
                let rev = doc.buffer.with(|b| b.rev());
                if last.is_none() || last == Some(rev) {
                    return rev;
                }
                let text = doc.buffer.with_untracked(|b| b.to_string());
                servers.update(|servers| {
                    if let Some(entry) =
                        servers.iter_mut().find(|(sid, _)| *sid == id)
                    {
                        entry.1.command = text;
                    }
                });
                let snapshot = servers.get_untracked();
                persist_mcp_servers(&snapshot);
                rev
            });

            let args_input = TextInputBuilder::new()
                .value(server.args.join(" "))
                .build(cx, editors, common.clone());
            let args_doc = args_input.doc_signal();
            create_effect(move |last| {
                let doc = args_doc.get_untracked();
                let rev = doc.buffer.with(|b| b.rev());
                if last.is_none() || last == Some(rev) {
                    return rev;
                }
                let text = doc.buffer.with_untracked(|b| b.to_string());
                servers.update(|servers| {
                    if let Some(entry) =
                        servers.iter_mut().find(|(sid, _)| *sid == id)
                    {
                        entry.1.args =
                            text.split_whitespace().map(|a| a.to_string()).collect();
                    }
                });
                let snapshot = servers.get_untracked();
                persist_mcp_servers(&snapshot);
                rev
            });

            let enabled = create_rw_signal(server.enabled);
            create_effect(move |last| {
                let enabled = enabled.get();
                if last.is_none() {
                    return;
                }
                servers.update(|servers| {
                    if let Some(entry) =
                        servers.iter_mut().find(|(sid, _)| *sid == id)
                    {
                        entry.1.enabled = enabled;
                    }
                });
                let snapshot = servers.get_untracked();
                persist_mcp_servers(&snapshot);
            });

            let field = |label_text: String, view: AnyView| -> AnyView {
                stack((
                    label(move || label_text.clone()).style(move |s| {
                        s.width(70.0)
                            .font_size(
                                (config.get().ui.font_size() as f32 - 1.0).max(11.0),
                            )
                            .color(config.get().color(LapceColor::EDITOR_DIM))
                    }),
                    view,
                ))
                .style(|s| {
                    s.items_center().col_gap(8.0).flex_grow(1.0).min_width(0.0)
                })
                .into_any()
            };

            stack((
                container(toggle_switch(move || enabled.get(), config))
                    .style(|s| s.items_center()),
                field(
                    "Name".to_string(),
                    name_input
                        .keyboard_navigable()
                        .style(move |s| {
                            s.width_pct(100.0)
                                .border(1.0)
                                .border_radius(6.0)
                                .border_color(
                                    config.get().color(LapceColor::LAPCE_BORDER),
                                )
                        })
                        .into_any(),
                ),
                field(
                    "Command".to_string(),
                    command_input
                        .keyboard_navigable()
                        .style(move |s| {
                            s.width_pct(100.0)
                                .border(1.0)
                                .border_radius(6.0)
                                .border_color(
                                    config.get().color(LapceColor::LAPCE_BORDER),
                                )
                        })
                        .into_any(),
                ),
                field(
                    "Args".to_string(),
                    args_input
                        .keyboard_navigable()
                        .style(move |s| {
                            s.width_pct(100.0)
                                .border(1.0)
                                .border_radius(6.0)
                                .border_color(
                                    config.get().color(LapceColor::LAPCE_BORDER),
                                )
                        })
                        .into_any(),
                ),
                icon_button(
                    LapceIcons::CLOSE,
                    "Remove",
                    move || {
                        servers
                            .update(|servers| servers.retain(|(sid, _)| *sid != id));
                        let snapshot = servers.get_untracked();
                        persist_mcp_servers(&snapshot);
                    },
                    config,
                ),
            ))
            .style(|s| {
                s.items_center()
                    .col_gap(8.0)
                    .padding_horiz(14.0)
                    .padding_vert(6.0)
            })
        },
    )
    .style(|s| s.flex_col().width_pct(100.0));

    let empty_hint =
        label(|| "No MCP servers configured".to_string()).style(move |s| {
            s.padding_horiz(14.0)
                .padding_vert(8.0)
                .font_size((config.get().ui.font_size() as f32 - 1.0).max(11.0))
                .color(config.get().color(LapceColor::EDITOR_DIM))
                .apply_if(!servers.with(|s| s.is_empty()), |s| s.hide())
        });

    let add =
        container(label(|| "+ Add server".to_string()).style(move |s| {
            s.color(config.get().color(LapceColor::EDITOR_FOREGROUND))
        }))
        .on_click_stop(move |_| {
            let id = next_id.get_untracked();
            next_id.set(id + 1);
            servers.update(|servers| servers.push((id, McpServerConfig::default())));
        })
        .style(move |s| {
            let config = config.get();
            s.padding_horiz(14.0)
                .padding_vert(10.0)
                .border_top(1.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .cursor(CursorStyle::Pointer)
                .hover(|s| {
                    s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
                })
        });

    let body = stack((rows, empty_hint, add))
        .style(|s| s.flex_col().width_pct(100.0))
        .into_any();

    build_card(
        "MCP Servers".to_string(),
        "Model Context Protocol servers exposed to the agent".to_string(),
        body,
        config,
    )
}

/// Persist the MCP server list into the `ai.mcp-servers` toml array.
fn persist_mcp_servers(servers: &[(u64, McpServerConfig)]) {
    let list: Vec<serde_json::Value> = servers
        .iter()
        .map(|(_, s)| {
            serde_json::json!({
                "name": s.name,
                "command": s.command,
                "args": s.args,
                "enabled": s.enabled,
            })
        })
        .collect();
    if let Ok(value) =
        serde::Serialize::serialize(&list, toml_edit::ser::ValueSerializer::new())
    {
        LapceConfig::update_file("ai", "mcp-servers", value);
    }
}

fn dropdown_view(
    item: &SettingsItem,
    current_value: RwSignal<String>,
    dropdown: &DropdownInfo,
    expanded: RwSignal<bool>,
    window_size: RwSignal<Size>,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View + use<> {
    let window_origin = create_rw_signal(Point::ZERO);
    let size = create_rw_signal(Size::ZERO);
    let overlay_id = create_rw_signal(None);
    let dropdown_input_focus = create_rw_signal(false);
    let dropdown_scroll_focus = create_rw_signal(true);

    {
        let item = item.to_owned();
        let dropdown = dropdown.to_owned();
        create_effect(move |_| {
            if expanded.get() {
                let item = item.clone();
                let dropdown = dropdown.clone();
                let id = add_overlay(Point::ZERO, move |_| {
                    dropdown_scroll(
                        &item.clone(),
                        current_value,
                        &dropdown.clone(),
                        expanded,
                        dropdown_scroll_focus,
                        dropdown_input_focus,
                        window_origin,
                        size,
                        window_size,
                        config,
                    )
                });
                overlay_id.set(Some(id));
            } else if let Some(id) = overlay_id.get_untracked() {
                remove_overlay(id);
                overlay_id.set(None);
            }
        });
    }

    stack((
        label(move || current_value.get()).style(move |s| {
            s.text_ellipsis()
                .width_pct(100.0)
                .padding_horiz(10.0)
                .selectable(false)
        }),
        container(
            svg(move || {
                if expanded.get() {
                    config.get().ui_svg(LapceIcons::CLOSE)
                } else {
                    config.get().ui_svg(LapceIcons::DROPDOWN_ARROW)
                }
            })
            .style(move |s| {
                let config = config.get();
                let size = config.ui.icon_size() as f32;
                s.size(size, size)
                    .color(config.color(LapceColor::LAPCE_ICON_ACTIVE))
            }),
        )
        .style(|s| s.padding_right(4.0)),
    ))
    .on_click_stop(move |_| {
        expanded.update(|expanded| {
            *expanded = !*expanded;
        });
    })
    .on_move(move |point| {
        window_origin.set(point);
        if expanded.get_untracked() {
            expanded.set(false);
        }
    })
    .on_resize(move |rect| {
        size.set(rect.size());
    })
    .style(move |s| {
        s.items_center()
            .cursor(CursorStyle::Pointer)
            .border_color(config.get().color(LapceColor::LAPCE_BORDER))
            .border(1.0)
            .border_radius(6.0)
            .width(250.0)
            .line_height(1.8)
    })
    .keyboard_navigable()
    .on_event_stop(EventListener::FocusGained, move |_| {
        dropdown_input_focus.set(true);
    })
    .on_event_stop(EventListener::FocusLost, move |_| {
        dropdown_input_focus.set(false);
        if expanded.get_untracked() && !dropdown_scroll_focus.get_untracked() {
            expanded.set(false);
        }
    })
    .on_cleanup(move || {
        if let Some(id) = overlay_id.get_untracked() {
            remove_overlay(id);
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn dropdown_scroll(
    item: &SettingsItem,
    current_value: RwSignal<String>,
    dropdown: &DropdownInfo,
    expanded: RwSignal<bool>,
    dropdown_scroll_focus: RwSignal<bool>,
    dropdown_input_focus: RwSignal<bool>,
    window_origin: RwSignal<Point>,
    input_size: RwSignal<Size>,
    window_size: RwSignal<Size>,
    config: ReadSignal<Arc<LapceConfig>>,
) -> impl View + use<> {
    dropdown_scroll_focus.set(true);

    let kind = item.kind.clone();
    let field = item.field.clone();
    let view_fn = move |item_string: String| {
        let kind = kind.clone();
        let field = field.clone();
        let local_item_string = item_string.clone();
        label(move || local_item_string.clone())
            .on_click_stop(move |_| {
                current_value.set(item_string.clone());
                if let Ok(value) = serde::Serialize::serialize(
                    &item_string,
                    toml_edit::ser::ValueSerializer::new(),
                ) {
                    LapceConfig::update_file(&kind, &field, value);
                }
                expanded.set(false);
            })
            .style(move |s| {
                s.text_ellipsis().padding_horiz(10.0).hover(|s| {
                    s.cursor(CursorStyle::Pointer).background(
                        config.get().color(LapceColor::PANEL_HOVERED_BACKGROUND),
                    )
                })
            })
    };

    let items = dropdown.items.clone();

    let scroll_size = create_rw_signal(Size::ZERO);

    scroll({
        dyn_stack(move || items.clone(), |item| item.to_string(), view_fn)
            .style(|s| s.flex_col().width_pct(100.0).cursor(CursorStyle::Pointer))
    })
    .style(move |s| {
        s.width_pct(100.0)
            .max_height(200.0)
            .set(PropagatePointerWheel, false)
    })
    .keyboard_navigable()
    .request_focus(|| {})
    .on_event_stop(EventListener::FocusGained, move |_| {
        dropdown_scroll_focus.set(true);
    })
    .on_event_stop(EventListener::FocusLost, move |_| {
        dropdown_scroll_focus.set(false);
        if expanded.get_untracked() && !dropdown_input_focus.get_untracked() {
            expanded.set(false);
        }
    })
    .on_event_stop(EventListener::PointerMove, move |_| {})
    .on_event_stop(EventListener::PointerDown, move |_| {})
    .on_resize(move |rect| {
        scroll_size.set(rect.size());
    })
    .style(move |s| {
        let config = config.get();
        let window_origin = window_origin.get();
        let window_size = window_size.get();
        let input_size = input_size.get();
        let scroll_size = scroll_size.get();

        let x = if window_origin.x + scroll_size.width + 5.0 > window_size.width {
            window_size.width - scroll_size.width - 5.0
        } else {
            window_origin.x
        };

        let y = if window_origin.y + input_size.height + scroll_size.height + 5.0
            > window_size.height
        {
            window_origin.y - scroll_size.height + 1.0
        } else {
            window_origin.y + input_size.height - 1.0
        };

        s.width(250.0)
            .line_height(1.8)
            .font_size(config.ui.font_size() as f32)
            .font_family(config.ui.font_family.clone())
            .color(config.color(LapceColor::EDITOR_FOREGROUND))
            .background(config.color(LapceColor::EDITOR_BACKGROUND))
            .class(floem::views::scroll::Handle, |s| {
                s.background(config.color(LapceColor::LAPCE_SCROLL_BAR))
            })
            .border(1)
            .border_radius(6.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .box_shadow_blur(3.0)
            .box_shadow_color(config.color(LapceColor::LAPCE_DROPDOWN_SHADOW))
            .inset_left(x)
            .inset_top(y)
    })
}

//! Agent Assets page: Skills, MCPs, Subagents, Rules, Commands and Hooks.
//!
//! Mirrors Cursor's "Agent" settings page: a kind filter row, an import/new
//! toolbar and a card list with enable toggles, docs and delete actions.

use std::{path::PathBuf, rc::Rc, sync::Arc};

use devforge_agent::{AgentAsset, AssetKind, AssetScope};
use devforge_core::mode::Mode;
use floem::{
    AnyView, IntoView, View,
    action::open_file,
    file::{FileDialogOptions, FileSpec},
    keyboard::Modifiers,
    reactive::{
        ReadSignal, RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect,
        create_memo, create_rw_signal,
    },
    style::CursorStyle,
    views::{
        Decorators, container, dyn_container, dyn_stack, empty, label,
        scroll::scroll, stack, stack_from_iter, svg,
    },
};

use crate::{
    app::tooltip_label,
    command::CommandExecuted,
    config::{LapceConfig, color::LapceColor, icon::LapceIcons},
    keypress::{KeyPressFocus, condition::Condition},
    main_split::Editors,
    settings::toggle_switch,
    text_input::TextInputBuilder,
    window_tab::CommonData,
};

/// Filter tab state: `None` is the "All" tab.
type KindFilter = RwSignal<Option<AssetKind>>;

/// Workspace-scope toggle used by the import / new-asset actions.
type ScopeSignal = RwSignal<AssetScope>;

#[derive(Clone, Debug)]
struct AssetsData {
    common: Rc<CommonData>,
    assets: RwSignal<Vec<AgentAsset>>,
    filter: KindFilter,
    scope: ScopeSignal,
    status: RwSignal<String>,
    /// Mirrors `ai.hooks-enabled`; hooks spawn local processes, so this is off
    /// unless the user explicitly turns it on.
    hooks_enabled: RwSignal<bool>,
}

impl AssetsData {
    fn new(cx: floem::reactive::Scope, common: Rc<CommonData>) -> Self {
        let assets = create_rw_signal(load_assets(&common));
        let hooks_enabled =
            create_rw_signal(common.config.get_untracked().ai.hooks_enabled);
        let data = Self {
            common,
            assets,
            filter: create_rw_signal(None),
            scope: create_rw_signal(AssetScope::Project),
            status: create_rw_signal(String::new()),
            hooks_enabled,
        };
        let _ = cx;
        data
    }

    fn workspace_root(&self) -> Option<PathBuf> {
        match &self.common.workspace.kind {
            crate::workspace::LapceWorkspaceType::Local => {
                self.common.workspace.path.clone()
            }
            _ => None,
        }
    }

    fn reload(&self) {
        self.assets.set(load_assets(&self.common));
    }

    fn set_status(&self, text: impl Into<String>) {
        self.status.set(text.into());
    }

    /// Project scope needs an open folder workspace; fall back to User scope.
    fn effective_scope(&self) -> AssetScope {
        let scope = self.scope.get_untracked();
        if scope == AssetScope::Project && self.workspace_root().is_none() {
            AssetScope::User
        } else {
            scope
        }
    }

    fn create_new(&self, kind: AssetKind, name: &str) {
        let root = self.workspace_root();
        match devforge_agent::create_asset(
            kind,
            self.effective_scope(),
            root.as_deref(),
            name,
        ) {
            Ok(path) => {
                self.reload();
                self.set_status(format!("Created `{}`", path.display()));
                self.open_path(&path);
            }
            Err(e) => self.set_status(format!("Could not create asset: {e}")),
        }
    }

    fn import_file(&self, kind: AssetKind, path: PathBuf) {
        let root = self.workspace_root();
        match devforge_agent::import_asset(
            kind,
            self.effective_scope(),
            root.as_deref(),
            &path,
        ) {
            Ok(dest) => {
                self.reload();
                self.set_status(format!("Imported `{}`", dest.display()));
            }
            Err(e) => self.set_status(format!("Import failed: {e}")),
        }
    }

    fn import_dir(&self, kind: AssetKind, path: PathBuf) {
        let root = self.workspace_root();
        match devforge_agent::import_directory(
            kind,
            self.effective_scope(),
            root.as_deref(),
            &path,
        ) {
            Ok(created) => {
                self.reload();
                self.set_status(format!("Imported {} file(s)", created.len()));
            }
            Err(e) => self.set_status(format!("Import failed: {e}")),
        }
    }

    fn toggle(&self, path: &std::path::Path, enabled: bool) {
        match devforge_agent::set_asset_enabled(path, enabled) {
            Ok(()) => self.reload(),
            Err(e) => self.set_status(format!("Could not update asset: {e}")),
        }
    }

    fn delete(&self, asset: &AgentAsset) {
        match devforge_agent::delete_asset(&asset.path) {
            Ok(()) => {
                self.reload();
                self.set_status(format!("Deleted `{}`", asset.name));
            }
            Err(e) => self.set_status(format!("Delete failed: {e}")),
        }
    }

    fn open_path(&self, path: &std::path::Path) {
        self.common.internal_command.send(
            crate::command::InternalCommand::OpenFile {
                path: path.to_path_buf(),
            },
        );
    }

    /// Persist `ai.hooks-enabled` and ask the workbench to reload the config so
    /// the next agent run picks the new value up.
    fn set_hooks_enabled(&self, enabled: bool) {
        self.hooks_enabled.set(enabled);
        let value = toml_edit::Value::from(enabled);
        if LapceConfig::update_file("ai", "hooks-enabled", value).is_none() {
            self.set_status("Could not write hooks-enabled to settings.toml");
            return;
        }
        self.common
            .internal_command
            .send(crate::command::InternalCommand::ReloadConfig);
        self.set_status(if enabled {
            "Hooks enabled — hooks.json commands will run during agent runs"
        } else {
            "Hooks disabled — bundles are listed but never executed"
        });
    }
}

fn load_assets(common: &CommonData) -> Vec<AgentAsset> {
    let root = match &common.workspace.kind {
        crate::workspace::LapceWorkspaceType::Local => common.workspace.path.clone(),
        _ => None,
    };
    devforge_agent::discover_all(root.as_deref())
}

impl KeyPressFocus for AssetsData {
    fn get_mode(&self) -> Mode {
        Mode::Insert
    }

    fn check_condition(&self, condition: Condition) -> bool {
        matches!(condition, Condition::PanelFocus)
    }

    fn run_command(
        &self,
        _command: &crate::command::LapceCommand,
        _count: Option<usize>,
        _mods: Modifiers,
    ) -> CommandExecuted {
        CommandExecuted::No
    }

    fn receive_char(&self, _c: &str) {}
}

/// Entry point used by the editor tab content.
pub fn agent_assets_view(editors: Editors, common: Rc<CommonData>) -> impl View {
    let config = common.config;
    let cx = floem::reactive::Scope::current();
    let data = AssetsData::new(cx, common.clone());

    let tabs = kind_tabs(data.clone(), config);
    let toolbar = asset_toolbar(editors, data.clone(), config);
    let list = asset_list(data.clone(), config);
    let docs = asset_docs(data.clone(), config);
    let status = label(move || data.status.get()).style(move |s| {
        s.font_size((config.get().ui.font_size() as f32 - 1.0).max(11.0))
            .color(config.get().color(LapceColor::EDITOR_DIM))
            .margin_top(6.0)
    });

    stack((
        header(data.clone(), config),
        tabs,
        toolbar,
        scroll(
            stack((list, docs, status))
                .style(|s| s.flex_col().width_pct(100.0).padding_bottom(40.0)),
        )
        .style(|s| s.flex_grow(1.0).width_pct(100.0)),
    ))
    .style(|s| s.flex_col().size_pct(100.0, 100.0))
    .debug_name("Agent Assets")
}

fn header(data: AssetsData, config: ReadSignal<Arc<LapceConfig>>) -> AnyView {
    let scope_btn = |scope: AssetScope, label_text: &'static str| -> AnyView {
        let data = data.clone();
        container(label(move || label_text.to_string()).style(move |s| {
            let active = data.scope.get() == scope;
            s.font_size((config.get().ui.font_size() as f32 - 1.0).max(11.0))
                .color(if active {
                    config.get().color(LapceColor::EDITOR_FOREGROUND)
                } else {
                    config.get().color(LapceColor::EDITOR_DIM)
                })
        }))
        .on_click_stop(move |_| data.scope.set(scope))
        .style(move |s| {
            let active = data.scope.get() == scope;
            let config = config.get();
            s.padding_horiz(10.0)
                .padding_vert(4.0)
                .border_radius(6.0)
                .cursor(CursorStyle::Pointer)
                .apply_if(active, |s| {
                    s.background(config.color(LapceColor::PANEL_CURRENT_BACKGROUND))
                })
                .hover(|s| {
                    s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
                })
        })
        .into_any()
    };

    let open_user_dir = {
        let data = data.clone();
        move || {
            if let Some(dir) = devforge_agent::default_location(
                AssetKind::Skills,
                AssetScope::User,
                None,
            )
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            {
                let _ = std::fs::create_dir_all(&dir);
                let _ = open::that(&dir);
                data.set_status(format!("Opened {}", dir.display()));
            }
        }
    };

    stack((
        stack((
            label(|| "Agent Assets".to_string()).style(move |s| {
                s.font_bold()
                    .font_size(config.get().ui.font_size() as f32 + 4.0)
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
            label(|| {
                "Skills, MCPs, Subagents, Rules, Commands and Hooks available to the AI agent"
                    .to_string()
            })
            .style(move |s| {
                s.margin_top(4.0)
                    .color(config.get().color(LapceColor::EDITOR_DIM))
            }),
        ))
        .style(|s| s.flex_col().items_start().flex_grow(1.0).min_width(0.0)),
        stack((
            container(label(|| "Scope:".to_string()).style(move |s| {
                s.font_size((config.get().ui.font_size() as f32 - 1.0).max(11.0))
                    .color(config.get().color(LapceColor::EDITOR_DIM))
            }))
            .style(|s| s.items_center()),
            scope_btn(AssetScope::Project, "Project"),
            scope_btn(AssetScope::User, "User"),
            container(
                label(|| "Open folder".to_string()).style(move |s| {
                    s.color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                }),
            )
            .on_click_stop(move |_| open_user_dir())
            .style(move |s| {
                let config = config.get();
                s.padding_horiz(10.0)
                    .padding_vert(4.0)
                    .border_radius(6.0)
                    .cursor(CursorStyle::Pointer)
                    .hover(|s| {
                        s.background(
                            config.color(LapceColor::PANEL_HOVERED_BACKGROUND),
                        )
                    })
            }),
        ))
        .style(|s| s.items_center().col_gap(6.0)),
    ))
    .style(|s| {
        s.width_pct(100.0)
            .items_center()
            .padding_horiz(28.0)
            .padding_top(20.0)
            .padding_bottom(10.0)
    })
    .into_any()
}

/// Cursor-style filter row: All + one pill per asset kind.
fn kind_tabs(data: AssetsData, config: ReadSignal<Arc<LapceConfig>>) -> AnyView {
    let pill = move |kind: Option<AssetKind>, text: &'static str| -> AnyView {
        let data = data.clone();
        container(label(move || text.to_string()).style(move |s| {
            let active = data.filter.get() == kind;
            s.font_size((config.get().ui.font_size() as f32 - 1.0).max(11.0))
                .color(if active {
                    config.get().color(LapceColor::EDITOR_FOREGROUND)
                } else {
                    config.get().color(LapceColor::EDITOR_DIM)
                })
        }))
        .on_click_stop(move |_| data.filter.set(kind))
        .style(move |s| {
            let active = data.filter.get() == kind;
            let config = config.get();
            s.padding_horiz(12.0)
                .padding_vert(4.0)
                .border_radius(999.0)
                .border(1.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .cursor(CursorStyle::Pointer)
                .apply_if(active, |s| {
                    s.background(config.color(LapceColor::PANEL_CURRENT_BACKGROUND))
                })
                .hover(|s| {
                    s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
                })
        })
        .into_any()
    };

    let mut children: Vec<AnyView> = vec![pill(None, "All")];
    for kind in AssetKind::ALL {
        children.push(pill(Some(kind), kind.label()));
    }

    stack_from_iter(children)
        .style(|s| {
            s.width_pct(100.0)
                .padding_horiz(28.0)
                .padding_bottom(10.0)
                .col_gap(6.0)
                .flex_wrap(floem::style::FlexWrap::Wrap)
        })
        .into_any()
}

/// Import / create toolbar for the active kind.
fn asset_toolbar(
    editors: Editors,
    data: AssetsData,
    config: ReadSignal<Arc<LapceConfig>>,
) -> AnyView {
    let name_input = TextInputBuilder::new().build(
        floem::reactive::Scope::current(),
        editors,
        data.common.clone(),
    );
    let name_doc = name_input.doc_signal();

    let active_kind = create_memo(move |_| data.filter.get());

    let new_asset = {
        let data = data.clone();
        move || {
            let Some(kind) = data.filter.get_untracked() else {
                data.set_status("Pick a specific tab (Skills, Rules, …) first");
                return;
            };
            let text = name_doc
                .get_untracked()
                .buffer
                .with_untracked(|b| b.to_string());
            let name = text.trim().to_string();
            let name = if name.is_empty() {
                format!("new-{}", kind.id())
            } else {
                name
            };
            data.create_new(kind, &name);
        }
    };

    let import_file = {
        let data = data.clone();
        move || {
            let Some(kind) = data.filter.get_untracked() else {
                data.set_status("Pick a specific tab (Skills, Rules, …) first");
                return;
            };
            let scope = data.effective_scope();
            let root = data.workspace_root();
            let start =
                devforge_agent::default_location(kind, scope, root.as_deref())
                    .ok()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()));
            let mut options = FileDialogOptions::new().title(format!(
                "Import {} ({})",
                kind.label(),
                scope.label()
            ));
            options = match kind.extension() {
                "json" => options.allowed_types(vec![FileSpec {
                    name: "JSON",
                    extensions: &["json"],
                }]),
                _ => options.allowed_types(vec![FileSpec {
                    name: "Markdown",
                    extensions: &["md"],
                }]),
            };
            if let Some(start) = start {
                options = options.force_starting_directory(start);
            }
            let data = data.clone();
            open_file(options, move |files| {
                let Some(info) = files else {
                    return;
                };
                if let Some(path) = info.path.into_iter().next() {
                    data.import_file(kind, path);
                }
            });
        }
    };

    let import_dir = {
        let data = data.clone();
        move || {
            let Some(kind) = data.filter.get_untracked() else {
                data.set_status("Pick a specific tab (Skills, Rules, …) first");
                return;
            };
            let scope = data.effective_scope();
            let root = data.workspace_root();
            let start =
                devforge_agent::default_location(kind, scope, root.as_deref())
                    .ok()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()));
            let mut options = FileDialogOptions::new()
                .title(format!("Import {} folder", kind.label()))
                .select_directories();
            if let Some(start) = start {
                options = options.force_starting_directory(start);
            }
            let data = data.clone();
            open_file(options, move |files| {
                let Some(info) = files else {
                    return;
                };
                if let Some(path) = info.path.into_iter().next() {
                    data.import_dir(kind, path);
                }
            });
        }
    };

    let hint = label(move || match active_kind.get() {
        Some(kind) => kind.label().to_string(),
        None => "All assets".to_string(),
    })
    .style(move |s| {
        s.font_size((config.get().ui.font_size() as f32 - 1.0).max(11.0))
            .color(config.get().color(LapceColor::EDITOR_DIM))
    });

    // Hooks execute local shell commands, so the switch is explicit and
    // labelled rather than hidden behind a settings key.
    let hooks_toggle = {
        let data = data.clone();
        let hooks_switch = toggle_switch(
            {
                let data = data.clone();
                move || data.hooks_enabled.get()
            },
            config,
        );
        stack((
            container(hooks_switch).style(|s| s.items_center()),
            container(label(|| "Run hooks".to_string()).style(move |s| {
                s.font_size((config.get().ui.font_size() as f32 - 1.0).max(11.0))
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }))
            .on_click_stop(move |_| {
                let next = !data.hooks_enabled.get_untracked();
                data.set_hooks_enabled(next);
            })
            .style(|s| s.items_center().cursor(CursorStyle::Pointer)),
        ))
        .style(|s| s.items_center().col_gap(6.0))
    };

    stack((
        hint,
        name_input
            .keyboard_navigable()
            .style(move |s| {
                s.width(180.0)
                    .border(1.0)
                    .border_radius(6.0)
                    .border_color(config.get().color(LapceColor::LAPCE_BORDER))
            })
            .into_any(),
        toolbar_action(LapceIcons::ADD, "New", new_asset, config),
        toolbar_action(LapceIcons::FILE, "Import file", import_file, config),
        toolbar_action(
            LapceIcons::DIRECTORY_CLOSED,
            "Import folder",
            import_dir,
            config,
        ),
        empty().style(|s| s.flex_grow(1.0)),
        hooks_toggle,
    ))
    .style(|s| {
        s.width_pct(100.0)
            .items_center()
            .col_gap(8.0)
            .padding_horiz(28.0)
            .padding_bottom(10.0)
    })
    .into_any()
}

fn toolbar_action(
    icon: &'static str,
    title: &'static str,
    on_click: impl Fn() + 'static,
    config: ReadSignal<Arc<LapceConfig>>,
) -> AnyView {
    container(
        stack((
            svg(move || config.get().ui_svg(icon)).style(move |s| {
                let size = (config.get().ui.icon_size() as f32 - 2.0).max(12.0);
                s.size(size, size)
                    .color(config.get().color(LapceColor::LAPCE_ICON_ACTIVE))
            }),
            label(move || title.to_string()).style(move |s| {
                s.font_size((config.get().ui.font_size() as f32 - 1.0).max(11.0))
                    .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
            }),
        ))
        .style(|s| s.items_center().col_gap(6.0)),
    )
    .on_click_stop(move |_| on_click())
    .style(move |s| {
        let config = config.get();
        s.padding_horiz(10.0)
            .padding_vert(4.0)
            .border_radius(6.0)
            .border(1.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .cursor(CursorStyle::Pointer)
            .hover(|s| {
                s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
            })
    })
    .into_any()
}

/// Card list of the assets matching the active filter.
fn asset_list(data: AssetsData, config: ReadSignal<Arc<LapceConfig>>) -> AnyView {
    let list_data = data.clone();
    let card_data = data.clone();
    let style_data = data.clone();
    let hint_data = data.clone();
    let cards = dyn_stack(
        move || {
            let filter = list_data.filter.get();
            list_data
                .assets
                .get()
                .into_iter()
                .filter(|a| filter.is_none_or(|k| k == a.kind))
                .collect::<Vec<_>>()
        },
        |asset| asset.path.clone(),
        move |asset| asset_card(asset, card_data.clone(), config),
    )
    .style(|s| s.flex_col().width_pct(100.0).row_gap(8.0));

    let empty_hint = label(move || {
        let filter = hint_data.filter.get();
        let count = hint_data.assets.with(|a| {
            a.iter()
                .filter(|x| filter.is_none_or(|k| k == x.kind))
                .count()
        });
        if count == 0 {
            match filter {
                Some(kind) => format!(
                    "No {} yet — create one with New or Import above.",
                    kind.label()
                ),
                None => "No agent assets found yet.".to_string(),
            }
        } else {
            String::new()
        }
    })
    .style(move |s| {
        s.padding_horiz(28.0)
            .padding_vert(10.0)
            .color(config.get().color(LapceColor::EDITOR_DIM))
            .apply_if(
                style_data.assets.with(|a| {
                    let filter = style_data.filter.get_untracked();
                    !a.is_empty()
                        && a.iter().any(|x| filter.is_none_or(|k| k == x.kind))
                }),
                |s| s.hide(),
            )
    });

    stack((
        container(cards).style(|s| {
            s.flex_col()
                .width_pct(100.0)
                .padding_horiz(28.0)
                .max_width(760.0)
        }),
        empty_hint,
    ))
    .style(|s| s.flex_col().width_pct(100.0))
    .into_any()
}

fn asset_card(
    asset: AgentAsset,
    data: AssetsData,
    config: ReadSignal<Arc<LapceConfig>>,
) -> AnyView {
    let path = asset.path.clone();
    let name = asset.name.clone();
    let subtitle = asset.subtitle();
    let kind = asset.kind;
    let scope = asset.scope;

    let enabled = create_rw_signal(asset.enabled);
    let path_for_toggle = path.clone();
    let toggle_data = data.clone();
    create_effect(move |last| {
        let value = enabled.get();
        if last.is_none() {
            return;
        }
        toggle_data.toggle(&path_for_toggle, value);
    });

    let toggle = {
        let data = data.clone();
        let path = path.clone();
        container(
            label(move || if enabled.get() { "On" } else { "Off" }.to_string())
                .style(move |s| {
                    s.font_size((config.get().ui.font_size() as f32 - 2.0).max(10.0))
                        .color(if enabled.get() {
                            config.get().color(LapceColor::EDITOR_FOREGROUND)
                        } else {
                            config.get().color(LapceColor::EDITOR_DIM)
                        })
                }),
        )
        .on_click_stop(move |_| {
            let next = !enabled.get_untracked();
            enabled.set(next);
            data.toggle(&path, next);
        })
        .style(move |s| {
            let config = config.get();
            s.padding_horiz(8.0)
                .padding_vert(2.0)
                .border_radius(999.0)
                .border(1.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .cursor(CursorStyle::Pointer)
                .apply_if(enabled.get(), |s| {
                    s.background(config.color(LapceColor::PANEL_CURRENT_BACKGROUND))
                })
                .hover(|s| {
                    s.background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
                })
        })
        .into_any()
    };

    let open = {
        let data = data.clone();
        let path = path.clone();
        move || data.open_path(&path)
    };
    let remove = {
        let data = data.clone();
        let asset = asset.clone();
        move || data.delete(&asset)
    };

    let title_row = stack((
        label(move || name.clone()).style(move |s| {
            s.font_bold()
                .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
        }),
        container(label(move || kind.label().to_string()).style(move |s| {
            s.font_size((config.get().ui.font_size() as f32 - 2.0).max(10.0))
                .color(config.get().color(LapceColor::EDITOR_DIM))
        }))
        .style(move |s| {
            let config = config.get();
            s.padding_horiz(6.0)
                .padding_vert(1.0)
                .border_radius(4.0)
                .background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
        }),
        container(label(move || scope.label().to_string()).style(move |s| {
            s.font_size((config.get().ui.font_size() as f32 - 2.0).max(10.0))
                .color(config.get().color(LapceColor::EDITOR_DIM))
        }))
        .style(move |s| {
            let config = config.get();
            s.padding_horiz(6.0)
                .padding_vert(1.0)
                .border_radius(4.0)
                .background(config.color(LapceColor::PANEL_HOVERED_BACKGROUND))
        }),
    ))
    .style(|s| s.items_center().col_gap(8.0));

    let path_label = label({
        let path = path.display().to_string();
        move || path.clone()
    })
    .style(move |s| {
        s.margin_top(4.0)
            .font_size((config.get().ui.font_size() as f32 - 1.0).max(11.0))
            .color(config.get().color(LapceColor::EDITOR_DIM))
            .text_ellipsis()
    });

    let subtitle_for_label = subtitle.clone();
    let body = stack((
        title_row,
        label(move || subtitle_for_label.clone()).style(move |s| {
            s.margin_top(2.0)
                .font_size((config.get().ui.font_size() as f32 - 1.0).max(11.0))
                .color(config.get().color(LapceColor::EDITOR_DIM))
                .apply_if(subtitle.is_empty(), |s| s.hide())
        }),
        path_label,
    ))
    .style(|s| s.flex_col().flex_grow(1.0).min_width(0.0));

    stack((
        container(body).style(|s| s.flex_grow(1.0).min_width(0.0)),
        stack((
            toggle,
            icon_action(LapceIcons::AI_EDIT, "Open", open, config),
            icon_action(LapceIcons::CLOSE, "Delete", remove, config),
        ))
        .style(|s| s.items_center().col_gap(4.0)),
    ))
    .style(move |s| {
        let config = config.get();
        s.width_pct(100.0)
            .items_center()
            .col_gap(12.0)
            .padding_horiz(14.0)
            .padding_vert(10.0)
            .border(1.0)
            .border_radius(8.0)
            .border_color(config.color(LapceColor::LAPCE_BORDER))
            .background(config.color(LapceColor::PANEL_BACKGROUND))
    })
    .into_any()
}

fn icon_action(
    icon: &'static str,
    title: &'static str,
    on_click: impl Fn() + 'static,
    config: ReadSignal<Arc<LapceConfig>>,
) -> AnyView {
    let view = container(svg(move || config.get().ui_svg(icon)).style(move |s| {
        let size = (config.get().ui.icon_size() as f32 - 2.0).max(12.0);
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
    });
    tooltip_label(config, view, move || title.to_string()).into_any()
}

/// Documentation + example block for the active kind.
fn asset_docs(data: AssetsData, config: ReadSignal<Arc<LapceConfig>>) -> AnyView {
    let content = dyn_container(
        move || data.filter.get(),
        move |kind| {
            let config = config;
            let body = match kind {
                Some(kind) => format!(
                    "{}\n\nExample ({}):\n\n{}",
                    devforge_agent::asset_doc_hint(kind),
                    kind.label(),
                    devforge_agent::asset_template(kind, "my-asset")
                ),
                None => "Select a tab above to see the format and an example for that asset kind."
                    .to_string(),
            };
            floem::views::text(body)
                .style(move |s| {
                    s.font_family("monospace".to_string())
                        .font_size(
                            (config.get().ui.font_size() as f32 - 1.0).max(11.0),
                        )
                        .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
                })
                .into_any()
        },
    );
    let title = label(move || match data.filter.get() {
        Some(kind) => format!("{} documentation", kind.label()),
        None => "Documentation".to_string(),
    })
    .style(move |s| {
        s.font_bold()
            .margin_bottom(6.0)
            .color(config.get().color(LapceColor::EDITOR_FOREGROUND))
    });

    stack((
        title,
        container(content).style(move |s| {
            let config = config.get();
            s.width_pct(100.0)
                .padding(12.0)
                .border(1.0)
                .border_radius(8.0)
                .border_color(config.color(LapceColor::LAPCE_BORDER))
                .background(config.color(LapceColor::PANEL_BACKGROUND))
        }),
    ))
    .style(|s| {
        s.flex_col()
            .width_pct(100.0)
            .padding_horiz(28.0)
            .padding_top(18.0)
            .max_width(760.0)
    })
    .into_any()
}

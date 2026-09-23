use serde::{Deserialize, Serialize};
use structdesc::FieldNames;

#[derive(FieldNames, Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct CoreConfig {
    #[field_names(desc = "Enable modal editing (Vim-style keybindings)")]
    pub modal: bool,
    #[field_names(desc = "Color theme for the DevForge UI and editor")]
    pub color_theme: String,
    #[field_names(desc = "Icon theme used in the sidebar, tabs, and file explorer")]
    pub icon_theme: String,
    #[field_names(
        desc = "Use DevForge custom title bar instead of the OS native one (Linux, BSD, Windows)"
    )]
    pub custom_titlebar: bool,
    #[field_names(
        desc = "Only open files in the explorer with a double-click (single-click selects)"
    )]
    pub file_explorer_double_click: bool,
    #[field_names(
        desc = "Automatically reload a plugin when its configuration changes"
    )]
    pub auto_reload_plugin: bool,
}

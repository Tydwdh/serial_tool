//! Platform-neutral panel metadata registry.
//!
//! The registry describes which panels exist and their presentation metadata.
//! Rendering is dispatched by the UI host, so this module does not depend on
//! `WorkbenchApp` or a platform runtime.

use egui_material_icons::{
    MaterialIcon,
    icons::{
        ICON_CABLE, ICON_EXTENSION, ICON_HISTORY, ICON_SEND, ICON_SETTINGS, ICON_TERMINAL,
        ICON_USB, ICON_VIEW_IN_AR,
    },
};
use std::collections::{HashMap, HashSet};
use tool_panels::DynamicPanels;
use tool_panels::{
    PANEL_CHART, PANEL_DEVICES, PANEL_LOGS, PANEL_PLUGINS, PANEL_REPLAY, PANEL_SENDER,
    PANEL_SETTINGS, PANEL_TERMINAL, PanelId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuiltinPanel {
    Devices,
    Replay,
    Plugins,
    Settings,
    Terminal,
    Sender,
    Logs,
    Chart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PanelKind {
    Builtin(BuiltinPanel),
    Dynamic { suffix: String },
}

#[derive(Clone)]
pub(crate) struct PanelDef {
    pub(crate) id: PanelId,
    pub(crate) title: String,
    pub(crate) icon: MaterialIcon,
    pub(crate) kind: PanelKind,
}

#[derive(Default)]
pub(crate) struct PanelRegistry {
    defs: HashMap<PanelId, PanelDef>,
    disabled: HashSet<PanelId>,
}

impl PanelRegistry {
    pub(crate) fn builtin() -> Self {
        let mut registry = Self::default();
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_DEVICES),
            title: "设备".to_owned(),
            icon: ICON_USB,
            kind: PanelKind::Builtin(BuiltinPanel::Devices),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_SETTINGS),
            title: "设置".to_owned(),
            icon: ICON_SETTINGS,
            kind: PanelKind::Builtin(BuiltinPanel::Settings),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_TERMINAL),
            title: "接收".to_owned(),
            icon: ICON_TERMINAL,
            kind: PanelKind::Builtin(BuiltinPanel::Terminal),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_REPLAY),
            title: "回放".to_owned(),
            icon: ICON_HISTORY,
            kind: PanelKind::Builtin(BuiltinPanel::Replay),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_PLUGINS),
            title: "插件".to_owned(),
            icon: ICON_EXTENSION,
            kind: PanelKind::Builtin(BuiltinPanel::Plugins),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_SENDER),
            title: "发送器".to_owned(),
            icon: ICON_SEND,
            kind: PanelKind::Builtin(BuiltinPanel::Sender),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_LOGS),
            title: "日志".to_owned(),
            icon: ICON_VIEW_IN_AR,
            kind: PanelKind::Builtin(BuiltinPanel::Logs),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_CHART),
            title: "图表".to_owned(),
            icon: ICON_VIEW_IN_AR,
            kind: PanelKind::Builtin(BuiltinPanel::Chart),
        });
        registry
    }

    fn register(&mut self, def: PanelDef) {
        self.defs.insert(def.id.clone(), def);
    }

    pub(crate) fn contains(&self, id: &PanelId) -> bool {
        self.defs.contains_key(id)
    }

    pub(crate) fn is_available(&self, id: &PanelId) -> bool {
        !self.disabled.contains(id)
    }

    pub(crate) fn title(&self, id: &PanelId) -> String {
        self.defs
            .get(id)
            .map(|def| def.title.clone())
            .unwrap_or_else(|| id.to_string())
    }

    pub(crate) fn icon(&self, id: &PanelId) -> MaterialIcon {
        self.defs.get(id).map(|def| def.icon).unwrap_or(ICON_CABLE)
    }

    pub(crate) fn kind_for(&self, id: &PanelId) -> Option<PanelKind> {
        self.defs.get(id).map(|def| def.kind.clone())
    }

    pub(crate) fn sync_dynamic_panels(&mut self, dynamic: &DynamicPanels) {
        self.defs.retain(|id, _| !id.is_dynamic());
        for id in dynamic.ids() {
            let panel_id = PanelId::dynamic(id);
            let title = dynamic.title(id).unwrap_or(id).to_owned();
            self.defs.insert(
                panel_id.clone(),
                PanelDef {
                    id: panel_id,
                    title,
                    icon: ICON_CABLE,
                    kind: PanelKind::Dynamic {
                        suffix: id.to_owned(),
                    },
                },
            );
        }
    }
}

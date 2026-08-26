//! Platform-neutral panel metadata registry.
//!
//! The registry describes which panels exist, their presentation metadata and
//! the capabilities they require. Rendering is dispatched by the UI host, so
//! this module does not depend on `WorkbenchApp` or a platform runtime.

use egui_material_icons::{
    MaterialIcon,
    icons::{
        ICON_CABLE, ICON_EXTENSION, ICON_HISTORY, ICON_SEND, ICON_SETTINGS, ICON_TERMINAL,
        ICON_USB, ICON_VIEW_IN_AR,
    },
};
use std::collections::{HashMap, HashSet};
use tool_application::{AppCapabilities, Capability};
use tool_panels::DynamicPanels;
use tool_panels::{
    PANEL_CHART, PANEL_DEVICES, PANEL_LOGS, PANEL_PLUGINS, PANEL_REPLAY, PANEL_SENDER,
    PANEL_SETTINGS, PANEL_TERMINAL, PanelId,
};

#[allow(dead_code)]
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
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
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
    required_capability: Option<Capability>,
}

pub(crate) struct PanelRegistry {
    defs: HashMap<PanelId, PanelDef>,
    capabilities: AppCapabilities,
    disabled: HashSet<PanelId>,
}

impl Default for PanelRegistry {
    fn default() -> Self {
        Self {
            defs: HashMap::new(),
            capabilities: AppCapabilities::native(),
            disabled: HashSet::new(),
        }
    }
}

impl PanelRegistry {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn builtin() -> Self {
        Self::for_capabilities(AppCapabilities::native())
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn web() -> Self {
        let registry = Self::for_capabilities(AppCapabilities::web());
        // Web keeps the native Dock positions. Panels whose capability has no
        // browser equivalent yet remain explicitly unavailable; implemented
        // panels use the same registry/layout rather than a second shell.
        registry
    }

    pub(crate) fn for_capabilities(capabilities: AppCapabilities) -> Self {
        let mut registry = Self {
            capabilities,
            ..Self::default()
        };
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_DEVICES),
            title: "设备".to_owned(),
            icon: ICON_USB,
            kind: PanelKind::Builtin(BuiltinPanel::Devices),
            required_capability: Some(Capability::Serial),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_SETTINGS),
            title: "设置".to_owned(),
            icon: ICON_SETTINGS,
            kind: PanelKind::Builtin(BuiltinPanel::Settings),
            required_capability: None,
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_TERMINAL),
            title: "接收".to_owned(),
            icon: ICON_TERMINAL,
            kind: PanelKind::Builtin(BuiltinPanel::Terminal),
            required_capability: None,
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_REPLAY),
            title: "回放".to_owned(),
            icon: ICON_HISTORY,
            kind: PanelKind::Builtin(BuiltinPanel::Replay),
            required_capability: Some(Capability::Replay),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_PLUGINS),
            title: "插件".to_owned(),
            icon: ICON_EXTENSION,
            kind: PanelKind::Builtin(BuiltinPanel::Plugins),
            required_capability: Some(Capability::Plugins),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_SENDER),
            title: "发送器".to_owned(),
            icon: ICON_SEND,
            kind: PanelKind::Builtin(BuiltinPanel::Sender),
            required_capability: Some(Capability::Serial),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_LOGS),
            title: "日志".to_owned(),
            icon: ICON_VIEW_IN_AR,
            kind: PanelKind::Builtin(BuiltinPanel::Logs),
            required_capability: None,
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_CHART),
            title: "图表".to_owned(),
            icon: ICON_VIEW_IN_AR,
            kind: PanelKind::Builtin(BuiltinPanel::Chart),
            required_capability: None,
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
            && self
                .defs
                .get(id)
                .and_then(|def| def.required_capability)
                .is_none_or(|capability| self.capabilities.supports(capability))
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
                    required_capability: None,
                },
            );
        }
    }
}

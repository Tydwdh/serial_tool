//! 统一面板注册表（PanelRegistry）。
//!
//! 所有面板——内置面板与插件动态面板——注册为同一份 [`PanelDef`]（标题、
//! 图标、渲染方式），布局树只保存 [`tool_panels::PanelId`]。渲染时按 id
//! 查询注册表统一分派，不再区分 built-in / dynamic。
//!
//! 与 [`crate::command_registry`] 同理：built-in 只是注册得比较早的普通
//! 面板，插件面板随插件启停动态同步。
//!
//! # 借用设计
//!
//! [`PanelDef`] 的渲染方式用可 Clone 的 [`PanelRender`] 表达：内置面板是
//! 函数指针，动态面板只携带裸 id。渲染前先 clone 出来再独占借用 `&mut
//! WorkbenchApp`，避免「从 `self.panel_registry` 借出 def 的同时又要
//! `&mut self`」的借用冲突。

use crate::app::WorkbenchApp;
use egui_material_icons::{
    MaterialIcon,
    icons::{
        ICON_CABLE, ICON_EXTENSION, ICON_HISTORY, ICON_SEND, ICON_SETTINGS, ICON_TERMINAL,
        ICON_USB, ICON_VIEW_IN_AR,
    },
};
use std::collections::HashMap;
use tool_panels::{
    DynamicPanels, PANEL_DEVICES, PANEL_LOGS, PANEL_PLUGINS, PANEL_REPLAY, PANEL_SENDER,
    PANEL_SETTINGS, PANEL_TERMINAL, PanelId,
};

/// 面板渲染方式。
#[derive(Clone)]
pub(crate) enum PanelRender {
    /// 内置面板：渲染函数。
    Builtin(fn(&mut WorkbenchApp, &mut egui::Ui)),
    /// 插件动态面板：裸 id（DynamicPanels 查询键）。
    Dynamic { suffix: String },
}

/// 一个面板的完整定义（id 即布局树中的 pane 标识）。
pub(crate) struct PanelDef {
    pub(crate) id: PanelId,
    pub(crate) title: String,
    pub(crate) icon: MaterialIcon,
    render: PanelRender,
}

/// 面板注册表：内置面板构造时注册；插件动态面板随插件启停动态同步。
#[derive(Default)]
pub(crate) struct PanelRegistry {
    defs: HashMap<PanelId, PanelDef>,
}

impl PanelRegistry {
    /// 注册全部内置面板。
    pub(crate) fn builtin() -> Self {
        let mut registry = Self::default();

        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_DEVICES),
            title: "设备".to_owned(),
            icon: ICON_USB,
            render: PanelRender::Builtin(render_devices),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_REPLAY),
            title: "回放".to_owned(),
            icon: ICON_HISTORY,
            render: PanelRender::Builtin(render_replay),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_PLUGINS),
            title: "插件".to_owned(),
            icon: ICON_EXTENSION,
            render: PanelRender::Builtin(render_plugins),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_SETTINGS),
            title: "设置".to_owned(),
            icon: ICON_SETTINGS,
            render: PanelRender::Builtin(render_settings),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_TERMINAL),
            title: "接收".to_owned(),
            icon: ICON_TERMINAL,
            render: PanelRender::Builtin(render_terminal),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_SENDER),
            title: "发送器".to_owned(),
            icon: ICON_SEND,
            render: PanelRender::Builtin(render_sender),
        });
        registry.register(PanelDef {
            id: PanelId::builtin(PANEL_LOGS),
            title: "日志".to_owned(),
            icon: ICON_VIEW_IN_AR,
            render: PanelRender::Builtin(render_logs),
        });

        registry
    }

    fn register(&mut self, def: PanelDef) {
        self.defs.insert(def.id.clone(), def);
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

    /// 渲染方式（clone 后调用，避免借用冲突；见模块文档）。
    pub(crate) fn render_for(&self, id: &PanelId) -> Option<PanelRender> {
        self.defs.get(id).map(|def| def.render.clone())
    }

    /// 同步插件动态面板到注册表：先移除全部动态面板定义，再按当前
    /// DynamicPanels 重建。内置面板不受影响。
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
                    render: PanelRender::Dynamic {
                        suffix: id.to_owned(),
                    },
                },
            );
        }
    }
}

// ── 内置面板渲染函数 ──

fn render_devices(app: &mut WorkbenchApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .id_salt("scroll-devices")
        .show(ui, |ui| app.device_panel(ui));
}

fn render_replay(app: &mut WorkbenchApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .id_salt("scroll-replay")
        .show(ui, |ui| app.replay_panel.ui(ui));
}

fn render_plugins(app: &mut WorkbenchApp, ui: &mut egui::Ui) {
    let events = egui::ScrollArea::vertical()
        .id_salt("scroll-plugins")
        .show(ui, |ui| app.plugins_panel.ui(ui, &mut app.plugin_manager))
        .inner;
    app.handle_plugin_panel_events(events);
}

fn render_settings(app: &mut WorkbenchApp, ui: &mut egui::Ui) {
    egui::ScrollArea::vertical()
        .id_salt("scroll-settings")
        .show(ui, |ui| app.settings_panel(ui));
}

fn render_terminal(app: &mut WorkbenchApp, ui: &mut egui::Ui) {
    // 每帧同步端口别名：别名变更可能发生在 device_panel / settings_panel 等多处，
    // 渲染前统一注入最简单可靠（别名数量少，clone 开销可忽略）。
    app.terminal_panel
        .set_port_aliases(&app.serial.port_aliases);
    app.terminal_panel.ui(ui);
}

fn render_sender(app: &mut WorkbenchApp, ui: &mut egui::Ui) {
    if ui.available_width() < 420.0 {
        app.send_panel_vertical(ui);
    } else {
        app.send_panel_horizontal(ui);
    }
}

fn render_logs(app: &mut WorkbenchApp, ui: &mut egui::Ui) {
    app.bottom_log_panel.ui(ui);
}

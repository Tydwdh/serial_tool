//! Shared serial UI primitives used by the Native and Web composition roots.
//!
//! The panel deliberately knows nothing about `Workbench`, `WebApplication`,
//! or a concrete transport. It renders a [`SerialView`] and returns
//! [`SerialAction`] values for the composition root to dispatch.

use egui::Ui;
use egui_material_icons::icons::{
    ICON_CABLE, ICON_CHEVRON_RIGHT, ICON_EXPAND_MORE, ICON_LINK_OFF, ICON_POWER_SETTINGS_NEW,
    ICON_REFRESH,
};
use std::collections::BTreeMap;
use tool_platform::{SerialParity, SerialSettings, TransportCapabilities};

// The grouped row contains status, port name, alias editor and group selector.
// Below this width it is rendered as two explicit rows instead of relying on
// `horizontal_wrapped`, whose nested layout can place the wrapped editor on
// top of the status row in narrow docks.
const PORT_INLINE_MIN_WIDTH: f32 = 560.0;

/// 未归入任何分组的端口所在的分组名，同时充当“该端口没有分组”的哨兵值。
const UNGROUPED: &str = "未分组";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerialPortItem {
    pub id: String,
    pub label: String,
    pub kind: String,
    pub open: bool,
    pub connecting: bool,
    pub pending_reconnect: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerialAction {
    Refresh,
    RequestPort,
    Connect {
        port: String,
        settings: SerialSettings,
    },
    Disconnect {
        port: String,
    },
    SendText {
        port: String,
        text: String,
    },
    SendHex {
        port: String,
        hex: String,
    },
    SetDtr {
        port: String,
        value: bool,
    },
    SetRts {
        port: String,
        value: bool,
    },
    CancelReconnect {
        port: String,
    },
    RemoveNetwork {
        port: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SerialTopBarAction {
    Refresh,
    RequestPort,
    Connect { port: String },
    Disconnect { port: String },
    Reconnect { port: String },
}

/// The platform-neutral serial controls shown in the application top bar.
///
/// Native and Web only adapt their transport state into this view and handle
/// the returned actions. Keeping the widget here prevents the two composition
/// roots from drifting in labels, spacing and button states.
pub struct SerialTopBarView<'a> {
    pub ports: &'a [SerialPortItem],
    pub aliases: &'a BTreeMap<String, String>,
    pub selected: &'a mut Option<String>,
    pub connected: Option<&'a str>,
    pub connecting: bool,
    pub collapsed: &'a mut bool,
    pub settings: SerialSettings,
    /// Web exposes the browser permission request from the shared top bar.
    /// Native keeps this disabled because its ports are already enumerated.
    pub show_request_port: bool,
}

/// Mutable UI state plus read-only transport data for one frame.
pub struct SerialView<'a> {
    pub ports: &'a [SerialPortItem],
    pub connected: Option<&'a str>,
    pub connecting: Option<&'a str>,
    pub status: &'a str,
    pub settings: &'a mut SerialSettings,
    pub send_input: &'a mut String,
    pub tx_hex: &'a mut bool,
    pub dtr: &'a mut bool,
    pub rts: &'a mut bool,
    pub capabilities: TransportCapabilities,
    /// Native keeps its richer grouped-port editor beside this shared core.
    pub show_ports: bool,
    /// The Native sender has a dedicated panel; Web keeps TX in Serial.
    pub show_sender: bool,
    /// Optional shared alias/group editor. Native can keep its richer device
    /// management surface, while Web uses this same row/group presentation
    /// instead of falling back to a flat port list.
    pub metadata: Option<SerialPortMetadata<'a>>,
}

pub struct SerialPortMetadata<'a> {
    pub aliases: &'a mut BTreeMap<String, String>,
    pub groups: &'a mut BTreeMap<String, String>,
}

/// `render_port_editor` 的输入。之所以收成结构体而不是继续用位置参数：
/// `alias` 与 `selected_group` 是相邻的两个 `&mut String`，在调用点调换它们
/// 编译不报错，却会把用户输入的别名写进分组表、把组名写进别名。按字段名
/// 书写后，写错会在调用点源码里自我暴露（`alias: &mut selected_group` 一眼
/// 看得见），并由本文件末尾的形状守卫机械挡住。
///
/// `metadata` 单独带一个 `'b`：`&mut` 对其内部生命周期是不变的，写成
/// `&'a mut SerialPortMetadata<'a>` 会要求元数据引用的那份数据活得和本函数
/// 调用点的局部 `alias` / `selected_group` 一样久，编译不过。两个生命周期
/// 正是原参数 `&mut SerialPortMetadata<'_>` 省略掉的形状。
struct PortEditorInputs<'a, 'b> {
    port: &'a SerialPortItem,
    alias: &'a mut String,
    selected_group: &'a mut String,
    group_names: &'a [String],
    metadata: &'a mut SerialPortMetadata<'b>,
    inline: bool,
}

pub struct SerialPanel;

impl SerialPanel {
    pub fn top_bar_contents_ui(
        ui: &mut Ui,
        view: &mut SerialTopBarView<'_>,
    ) -> Vec<SerialTopBarAction> {
        let mut actions = Vec::new();
        let ports = view.ports;
        let aliases = view.aliases;
        let selected = &mut *view.selected;
        let connected = view.connected;
        let connecting = view.connecting;
        let collapsed = &mut *view.collapsed;
        let settings = view.settings;
        let connected_label = connected
            .and_then(|id| ports.iter().find(|port| port.id == id))
            .map(|port| port_display_label(port, aliases))
            .unwrap_or_else(|| "未连接".to_owned());

        let state_color = if connecting {
            crate::theme::yellow()
        } else if connected.is_some() {
            crate::theme::green()
        } else {
            crate::theme::text_dimmed()
        };
        let label_color = if connected.is_some() {
            crate::theme::text_primary()
        } else {
            crate::theme::text_secondary()
        };
        // Linear 式连接 chip：中性表面 + 状态点，不再用红/绿整块染色。
        let chip = egui::Frame::new()
            .fill(crate::theme::bg_secondary())
            .stroke(egui::Stroke::new(1.0, crate::theme::border()))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(9, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(6.0, 6.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.0, state_color);
                    ui.label(
                        egui::RichText::new(&connected_label)
                            .size(11.5)
                            .strong()
                            .color(label_color),
                    );
                    if connected.is_some() {
                        ui.label(
                            egui::RichText::new(serial_settings_label(settings))
                                .size(9.5)
                                .color(crate::theme::text_dimmed()),
                        );
                    }
                    let chevron = if *collapsed {
                        ICON_CHEVRON_RIGHT.codepoint
                    } else {
                        ICON_EXPAND_MORE.codepoint
                    };
                    ui.label(
                        egui::RichText::new(chevron)
                            .size(14.0)
                            .color(crate::theme::text_dimmed()),
                    );
                });
            })
            .response
            .interact(egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if chip.clicked() {
            let was_collapsed = *collapsed;
            *collapsed = !*collapsed;
            if was_collapsed {
                actions.push(SerialTopBarAction::Refresh);
            }
        }

        if !*collapsed {
            let selected_text = selected
                .as_deref()
                .and_then(|id| ports.iter().find(|port| port.id == id))
                .map(|port| port_display_label(port, aliases))
                .or_else(|| selected.clone())
                .unwrap_or_else(|| {
                    if ports.is_empty() {
                        "无可用串口".to_owned()
                    } else {
                        "请选择串口".to_owned()
                    }
                });
            let combo_response = egui::ComboBox::from_id_salt("shared-top-port")
                .width(120.0)
                .selected_text(selected_text)
                .show_ui(ui, |ui| {
                    if ports.is_empty() {
                        ui.add_enabled(false, egui::Label::new("无可用串口"));
                    } else {
                        for port in ports {
                            ui.selectable_value(
                                selected,
                                Some(port.id.clone()),
                                port_display_label(port, aliases),
                            );
                        }
                    }
                });
            if combo_response.response.clicked() {
                actions.push(SerialTopBarAction::Refresh);
            }

            let selected_open = selected
                .as_deref()
                .is_some_and(|port| connected == Some(port));
            if !connecting && selected_open {
                if crate::design::icon_button(ui, ICON_REFRESH, "重连").clicked()
                    && let Some(port) = selected.clone()
                {
                    actions.push(SerialTopBarAction::Reconnect { port });
                }
            } else if connecting {
                ui.add_enabled_ui(false, |ui| {
                    let _ = crate::design::button(
                        ui,
                        ICON_POWER_SETTINGS_NEW,
                        "连接中",
                        crate::design::ButtonKind::Ghost,
                    );
                });
            } else if let Some(port) = selected.clone() {
                if crate::design::button(
                    ui,
                    ICON_POWER_SETTINGS_NEW,
                    "打开",
                    crate::design::ButtonKind::Primary,
                )
                .clicked()
                {
                    actions.push(SerialTopBarAction::Connect { port });
                }
            } else {
                let _ = ui.add_enabled(
                    false,
                    egui::Button::new(crate::design::icon_text(ICON_POWER_SETTINGS_NEW, "打开")),
                );
            }

            let close = ui
                .add_enabled_ui(selected_open, |ui| {
                    crate::design::button(
                        ui,
                        ICON_LINK_OFF,
                        "关闭",
                        crate::design::ButtonKind::Secondary,
                    )
                })
                .inner;
            let mut close = close;
            if !selected_open {
                close = close.on_disabled_hover_text("端口未打开");
            }
            if close.clicked()
                && let Some(port) = selected.clone()
            {
                actions.push(SerialTopBarAction::Disconnect { port });
            }
        }

        if view.show_request_port
            && crate::design::button(ui, ICON_CABLE, "添加设备", crate::design::ButtonKind::Ghost)
                .clicked()
        {
            actions.push(SerialTopBarAction::RequestPort);
        }

        actions
    }

    pub fn ui(ui: &mut Ui, view: &mut SerialView<'_>) -> Vec<SerialAction> {
        ui.heading("串口");
        ui.separator();
        Self::settings_ui(ui, view.settings);

        let mut actions = Self::port_list_ui(ui, view);
        if view.show_sender {
            Self::sender_ui(ui, view, &mut actions);
        }
        Self::signal_ui(ui, view, &mut actions);
        actions
    }

    /// Render the shared refresh/status/port-list section.
    ///
    /// Kept independently callable so the device panel can place it after the
    /// connection controls without duplicating the list logic.
    pub fn port_list_ui(ui: &mut Ui, view: &mut SerialView<'_>) -> Vec<SerialAction> {
        let mut actions = Vec::new();

        if !view.status.is_empty() {
            ui.label(view.status);
        }

        if view.show_ports && view.metadata.is_some() {
            actions.extend(Self::grouped_port_list_ui(ui, view));
            return actions;
        }

        if view.show_ports {
            for port in view.ports {
                let connecting = port.connecting;
                ui.horizontal_wrapped(|ui| {
                    ui.label(&port.label);
                    if !port.kind.is_empty() {
                        ui.weak(port.kind.as_str());
                    }
                    if port.label != port.id {
                        ui.monospace(&port.id);
                    }
                    if port.open {
                        if ui
                            .add_enabled(view.capabilities.disconnect, egui::Button::new("断开"))
                            .clicked()
                        {
                            actions.push(SerialAction::Disconnect {
                                port: port.id.clone(),
                            });
                        }
                    } else if ui
                        .add_enabled(
                            !connecting && view.capabilities.connect,
                            egui::Button::new(if connecting { "连接中" } else { "连接" }),
                        )
                        .clicked()
                    {
                        actions.push(SerialAction::Connect {
                            port: port.id.clone(),
                            settings: *view.settings,
                        });
                    }
                });
            }
        }

        actions
    }

    pub fn grouped_port_list_ui(ui: &mut Ui, view: &mut SerialView<'_>) -> Vec<SerialAction> {
        let mut metadata = view
            .metadata
            .take()
            .expect("metadata was checked before rendering grouped ports");
        let mut actions = Vec::new();
        let groups = Self::collect_port_groups(view.ports, metadata.groups);
        let group_names: Vec<String> = groups
            .keys()
            .filter(|name| name.as_str() != UNGROUPED)
            .cloned()
            .collect();

        for (group_name, ports) in groups {
            let open = Self::render_port_group_header(ui, &group_name, ports.len(), &mut actions);

            if open {
                for port in ports {
                    Self::render_grouped_port(
                        ui,
                        view,
                        port,
                        &group_name,
                        &group_names,
                        &mut metadata,
                        &mut actions,
                    );
                }
            }
        }

        let mut rename_group = None;
        let mut delete_group = None;
        if !group_names.is_empty() {
            ui.collapsing("分组管理", |ui| {
                for group in &group_names {
                    let edit_id = ui.make_persistent_id(("shared-port-rename", group));
                    let mut replacement = ui.ctx().data_mut(|data| {
                        data.get_persisted::<String>(edit_id)
                            .unwrap_or_else(|| group.clone())
                    });
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut replacement).desired_width(140.0));
                        if ui.small_button("改名").clicked()
                            && !replacement.trim().is_empty()
                            && replacement.trim() != group
                        {
                            rename_group = Some((group.clone(), replacement.trim().to_owned()));
                        }
                        if ui.small_button("删除").clicked() {
                            delete_group = Some(group.clone());
                        }
                    });
                    ui.ctx()
                        .data_mut(|data| data.insert_persisted(edit_id, replacement));
                }
            });
        }

        Self::render_new_group_row(ui, view, &mut metadata);

        if let Some(group) = delete_group {
            metadata.groups.retain(|_, value| value != &group);
        }
        if let Some((old_group, new_group)) = rename_group {
            for value in metadata.groups.values_mut() {
                if value == &old_group {
                    *value = new_group.clone();
                }
            }
        }

        view.metadata = Some(metadata);
        actions
    }

    /// 按元数据里的分组归属把端口归拢；没有分组的端口统一落进“未分组”。
    fn collect_port_groups<'p>(
        ports: &'p [SerialPortItem],
        assignments: &BTreeMap<String, String>,
    ) -> BTreeMap<String, Vec<&'p SerialPortItem>> {
        let mut groups: BTreeMap<String, Vec<&SerialPortItem>> = BTreeMap::new();
        for port in ports {
            let group = assignments
                .get(&port.id)
                .cloned()
                .unwrap_or_else(|| UNGROUPED.to_owned());
            groups.entry(group).or_default().push(port);
        }
        groups
    }

    /// 渲染分组标题行（折叠箭头 + 组名 + 端口数），返回该分组本帧是否展开。
    ///
    /// 展开态以组名为 key 持久化在 egui 的内存数据里，跨帧保留。
    fn render_port_group_header(
        ui: &mut Ui,
        group_name: &str,
        port_count: usize,
        actions: &mut Vec<SerialAction>,
    ) -> bool {
        let group_id = ui.make_persistent_id(("shared-port-group", group_name));
        let mut open = ui
            .ctx()
            .data_mut(|data| data.get_persisted::<bool>(group_id).unwrap_or(true));
        ui.horizontal_wrapped(|ui| {
            if ui
                // Keep the release layout's plain arrows. Material icon
                // glyphs are not guaranteed to be present in a browser's
                // fallback font and were rendered as square boxes in Web.
                .selectable_label(false, if open { "▾" } else { "▸" })
                .clicked()
            {
                open = !open;
                actions.push(SerialAction::Refresh);
            }
            ui.label(
                egui::RichText::new(group_name).color(if group_name == UNGROUPED {
                    crate::theme::text_secondary()
                } else {
                    crate::theme::text_primary()
                }),
            );
            ui.label(
                egui::RichText::new(format!("({port_count})")).color(crate::theme::text_dimmed()),
            );
        });
        ui.ctx()
            .data_mut(|data| data.insert_persisted(group_id, open));
        open
    }

    /// 渲染分组里的一个端口行：状态控件 + 别名/分组编辑器。
    ///
    /// 可用宽度够（见 `PORT_INLINE_MIN_WIDTH`）就走单行内联，否则拆成显式两行。
    fn render_grouped_port(
        ui: &mut Ui,
        view: &SerialView<'_>,
        port: &SerialPortItem,
        group_name: &str,
        group_names: &[String],
        metadata: &mut SerialPortMetadata<'_>,
        actions: &mut Vec<SerialAction>,
    ) {
        let mut alias = metadata.aliases.get(&port.id).cloned().unwrap_or_default();
        let mut selected_group = group_name.to_owned();
        let inline = ui.available_width() >= PORT_INLINE_MIN_WIDTH;
        let capabilities = view.capabilities;
        let settings = *view.settings;
        if inline {
            // Keep this row non-wrapping. The width check above is
            // deliberately conservative so the editor never wraps
            // into the status controls.
            ui.horizontal(|ui| {
                Self::render_port_status(ui, port, capabilities, settings, actions);
                Self::render_port_editor(
                    ui,
                    &mut PortEditorInputs {
                        port,
                        alias: &mut alias,
                        selected_group: &mut selected_group,
                        group_names,
                        metadata,
                        inline: true,
                    },
                );
            });
        } else {
            // Narrow docks get an explicit second line. Do not use
            // nested `horizontal_wrapped` here: egui cannot always
            // account for its wrapped child height correctly.
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    Self::render_port_status(ui, port, capabilities, settings, actions);
                });
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    Self::render_port_editor(
                        ui,
                        &mut PortEditorInputs {
                            port,
                            alias: &mut alias,
                            selected_group: &mut selected_group,
                            group_names,
                            metadata,
                            inline: false,
                        },
                    );
                });
                ui.add_space(4.0);
            });
        }
    }

    /// 渲染“新建分组”行：选端口 → 填组名 → 把该端口 assign 进去。
    fn render_new_group_row(
        ui: &mut Ui,
        view: &SerialView<'_>,
        metadata: &mut SerialPortMetadata<'_>,
    ) {
        // A new group is created by assigning it to a selected port. This
        // keeps the persisted model small and mirrors Native's “group only
        // exists once it contains a port” behavior.
        let new_group_id = ui.make_persistent_id("shared-port-new-group-name");
        let new_port_id = ui.make_persistent_id("shared-port-new-group-port");
        let mut new_group = ui.ctx().data_mut(|data| {
            data.get_persisted::<String>(new_group_id)
                .unwrap_or_default()
        });
        let ports = view.ports;
        let mut new_port = ui.ctx().data_mut(|data| {
            data.get_persisted::<String>(new_port_id)
                .or_else(|| ports.first().map(|port| port.id.clone()))
                .unwrap_or_default()
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("新建分组");
            egui::ComboBox::from_id_salt("shared-port-new-group-port")
                .selected_text(if new_port.is_empty() {
                    "选择端口"
                } else {
                    new_port.as_str()
                })
                .show_ui(ui, |ui| {
                    for port in ports {
                        ui.selectable_value(&mut new_port, port.id.clone(), &port.id);
                    }
                });
            ui.add(
                egui::TextEdit::singleline(&mut new_group)
                    .desired_width(130.0)
                    .hint_text("组名"),
            );
            if ui
                .add_enabled(
                    !new_port.is_empty() && !new_group.trim().is_empty(),
                    egui::Button::new("添加"),
                )
                .clicked()
            {
                metadata
                    .groups
                    .insert(new_port.clone(), new_group.trim().to_owned());
                new_group.clear();
            }
        });
        ui.ctx()
            .data_mut(|data| data.insert_persisted(new_group_id, new_group));
        ui.ctx()
            .data_mut(|data| data.insert_persisted(new_port_id, new_port));
    }

    /// Render the stable, non-wrapping portion of one grouped port row.
    fn render_port_status(
        ui: &mut Ui,
        port: &SerialPortItem,
        capabilities: TransportCapabilities,
        settings: SerialSettings,
        actions: &mut Vec<SerialAction>,
    ) {
        let (status_label, tooltip) = if port.pending_reconnect {
            ("⟳连", "重连中，点击取消")
        } else if port.open {
            ("●开", "已打开，点击关闭")
        } else if port.connecting {
            ("◌中", "连接中")
        } else {
            ("○关", "未打开，点击打开")
        };
        // The colour chain is deliberately not merged with the label chain
        // above: it treats "connecting" as yellow together with pending
        // reconnects, so it must stay a separate decision.
        let status_color = if port.pending_reconnect || port.connecting {
            crate::theme::yellow()
        } else if port.open {
            crate::theme::green()
        } else {
            crate::theme::red()
        };
        if ui
            .add(
                egui::Button::new(
                    egui::RichText::new(status_label)
                        .color(status_color)
                        .small(),
                )
                .frame(false)
                .min_size(egui::vec2(36.0, 18.0)),
            )
            .on_hover_text(tooltip)
            .clicked()
        {
            if port.pending_reconnect {
                actions.push(SerialAction::CancelReconnect {
                    port: port.id.clone(),
                });
            } else if port.open && capabilities.disconnect {
                actions.push(SerialAction::Disconnect {
                    port: port.id.clone(),
                });
            } else if !port.connecting && capabilities.connect {
                actions.push(SerialAction::Connect {
                    port: port.id.clone(),
                    settings,
                });
            }
        }
        ui.monospace(if port.label.trim().is_empty() {
            port.id.as_str()
        } else {
            port.label.as_str()
        })
        .on_hover_text(&port.id);
        if !port.kind.is_empty() {
            ui.label(&port.kind);
        }
        if port.kind == "网络" && ui.small_button("×").on_hover_text("移除网络端口").clicked()
        {
            actions.push(SerialAction::RemoveNetwork {
                port: port.id.clone(),
            });
        }
    }

    fn render_port_editor(ui: &mut Ui, inputs: &mut PortEditorInputs<'_, '_>) {
        let port = inputs.port;
        let alias = &mut *inputs.alias;
        let selected_group = &mut *inputs.selected_group;
        let group_names = inputs.group_names;
        let metadata = &mut *inputs.metadata;
        let inline = inputs.inline;
        ui.label("别名");
        let alias_width = if inline {
            (ui.available_width() - 150.0).clamp(120.0, 240.0)
        } else {
            (ui.available_width() - 150.0).clamp(100.0, 220.0)
        };
        let alias_response = ui.add(
            egui::TextEdit::singleline(alias)
                .desired_width(alias_width)
                .hint_text("例如 主控板"),
        );
        if alias_response.changed() {
            if alias.trim().is_empty() {
                metadata.aliases.remove(&port.id);
            } else {
                metadata
                    .aliases
                    .insert(port.id.clone(), alias.trim().to_owned());
            }
        }

        let group_width = if inline {
            125.0
        } else {
            ui.available_width().clamp(80.0, 125.0)
        };
        egui::ComboBox::from_id_salt(("shared-port-group-select", &port.id))
            .selected_text(selected_group.as_str())
            .width(group_width)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_value(selected_group, UNGROUPED.to_owned(), UNGROUPED)
                    .changed()
                {
                    metadata.groups.remove(&port.id);
                }
                for group in group_names {
                    if ui
                        .selectable_value(selected_group, group.clone(), group)
                        .changed()
                    {
                        metadata.groups.insert(port.id.clone(), group.clone());
                    }
                }
            });
    }

    /// Render DTR/RTS controls for the currently connected port.
    pub fn signal_ui(ui: &mut Ui, view: &mut SerialView<'_>, actions: &mut Vec<SerialAction>) {
        if let Some(port) = view.connected {
            ui.horizontal_wrapped(|ui| {
                let dtr_changed = ui.checkbox(view.dtr, "DTR").changed();
                if dtr_changed && view.capabilities.set_dtr {
                    actions.push(SerialAction::SetDtr {
                        port: port.to_owned(),
                        value: *view.dtr,
                    });
                }
                let rts_changed = ui.checkbox(view.rts, "RTS").changed();
                if rts_changed && view.capabilities.set_rts {
                    actions.push(SerialAction::SetRts {
                        port: port.to_owned(),
                        value: *view.rts,
                    });
                }
            });
        }
    }

    pub fn settings_ui(ui: &mut Ui, settings: &mut SerialSettings) {
        // The settings row is also used inside the shared left Dock. At
        // roughly 500px of usable width four default ComboBox widths are
        // enough to push parity outside the card. Keep the same one-row
        // composition with a compact width breakpoint instead of relying on
        // horizontal_wrapped to hide the last control below the next section.
        let combo_width = if ui.available_width() < 620.0 {
            88.0
        } else {
            118.0
        };
        ui.horizontal_wrapped(|ui| {
            // 四个参数各带字段名；原先只有最前面的「串口参数」一个标签，
            // 读起来像「串口参数 115200 8 1 无」一整串数值。
            ui.label("波特率");
            crate::design::combo_slot(ui, combo_width, |ui| {
                egui::ComboBox::from_id_salt("shared-serial-baud-rate")
                    .width(combo_width)
                    .selected_text(settings.baud_rate.to_string())
                    .show_ui(ui, |ui| {
                        for baud_rate in [
                            9_600, 19_200, 38_400, 57_600, 115_200, 230_400, 460_800, 921_600,
                            1_000_000, 2_000_000, 3_000_000,
                        ] {
                            ui.selectable_value(
                                &mut settings.baud_rate,
                                baud_rate,
                                baud_rate.to_string(),
                            );
                        }
                    });
            });
            ui.label("数据位");
            crate::design::combo_slot(ui, combo_width, |ui| {
                egui::ComboBox::from_id_salt("shared-serial-data-bits")
                    .width(combo_width)
                    .selected_text(settings.data_bits.to_string())
                    .show_ui(ui, |ui| {
                        for data_bits in [5, 6, 7, 8] {
                            ui.selectable_value(
                                &mut settings.data_bits,
                                data_bits,
                                data_bits.to_string(),
                            );
                        }
                    });
            });
            ui.label("停止位");
            crate::design::combo_slot(ui, combo_width, |ui| {
                egui::ComboBox::from_id_salt("shared-serial-stop-bits")
                    .width(combo_width)
                    .selected_text(settings.stop_bits.to_string())
                    .show_ui(ui, |ui| {
                        for stop_bits in [1, 2] {
                            ui.selectable_value(
                                &mut settings.stop_bits,
                                stop_bits,
                                stop_bits.to_string(),
                            );
                        }
                    });
            });
            ui.label("校验");
            crate::design::combo_slot(ui, combo_width, |ui| {
                egui::ComboBox::from_id_salt("shared-serial-parity")
                    .width(combo_width)
                    .selected_text(parity_label(settings.parity))
                    .show_ui(ui, |ui| {
                        for parity in [SerialParity::None, SerialParity::Odd, SerialParity::Even] {
                            ui.selectable_value(&mut settings.parity, parity, parity_label(parity));
                        }
                    });
            });
        });
    }

    pub fn sender_ui(ui: &mut Ui, view: &mut SerialView<'_>, actions: &mut Vec<SerialAction>) {
        let Some(port) = view.connected else {
            ui.colored_label(crate::theme::text_secondary(), "请先连接串口后再发送数据");
            return;
        };
        ui.horizontal(|ui| {
            ui.selectable_value(view.tx_hex, false, "TEXT");
            ui.selectable_value(view.tx_hex, true, "HEX");
            ui.text_edit_singleline(view.send_input);
            if ui
                .add_enabled(view.capabilities.send, egui::Button::new("发送"))
                .clicked()
            {
                if *view.tx_hex {
                    actions.push(SerialAction::SendHex {
                        port: port.to_owned(),
                        hex: view.send_input.clone(),
                    });
                } else {
                    actions.push(SerialAction::SendText {
                        port: port.to_owned(),
                        text: view.send_input.clone(),
                    });
                }
            }
        });
    }
}

fn parity_label(parity: SerialParity) -> &'static str {
    match parity {
        SerialParity::None => "无",
        SerialParity::Odd => "奇",
        SerialParity::Even => "偶",
    }
}

fn port_display_label(port: &SerialPortItem, aliases: &BTreeMap<String, String>) -> String {
    if let Some(alias) = aliases
        .get(&port.id)
        .map(|alias| alias.trim())
        .filter(|alias| !alias.is_empty())
    {
        return format!("{alias} ({})", port.id);
    }
    if port.label.trim().is_empty() || port.label == port.id {
        port.id.clone()
    } else {
        format!("{} ({})", port.label, port.id)
    }
}

fn serial_settings_label(settings: SerialSettings) -> String {
    let parity = match settings.parity {
        SerialParity::None => 'N',
        SerialParity::Odd => 'O',
        SerialParity::Even => 'E',
    };
    format!(
        "{} {}{}{}",
        settings.baud_rate, settings.data_bits, parity, settings.stop_bits
    )
}

#[cfg(test)]
mod tests {
    //! `render_port_editor` 的形状守卫。
    //!
    //! 说清楚它守什么、不守什么：`alias` 与 `selected_group` 是相邻的两个
    //! `&mut String`，收进 `PortEditorInputs` 之前，在调用点把它们写反是类型
    //! 全对、编译通过的错误，代价由用户付（敲进别名的文字进了分组表，反之亦
    //! 然）。结构体只消灭了**位置**错配这一种失误，风险挪到了字段赋值上，
    //! 所以这里机械地盯住赋值本身：显式写出的 `X: &mut Y` 必须 `X == Y`，并且
    //! 两处调用点各有一条。
    //!
    //! 两条被删掉的东西也钉住：那个只为“未来调用者”兜底的初值参数、以及它喂的
    //! 同步块不许回来 —— 这条断言扫的是整个文件连注释一起，所以连这里都不能写出
    //! 它的名字（名字在失败消息里由 `DEAD_SYNC` 展开）。函数体也不许被搬空后让
    //! 守卫继续绿。
    //!
    //! 实现上两个坑：被搜索的字面量一律用 `concat!` 拼，否则本模块自己的文本
    //! 会被当成命中（同 `crates/app/tests/lint_exemptions.rs`）；扫描前先按行
    //! 把注释与字符串的**内容**抹成空格，文档里引用这些字面量才不会误计。

    use std::fs;
    use std::path::Path;

    const EDITOR_FN: &str = concat!("fn ", "render_port_editor");
    const INPUTS_LITERAL: &str = concat!("PortEditorInputs", " {");
    const ALIAS_BINDING: &str = concat!("alias: &mut ", "alias,");
    const GROUP_BINDING: &str = concat!("selected_group: &mut ", "selected_group,");
    const DEAD_SYNC: &str = concat!("current", "_group");
    const TEST_MODULE: &str = concat!("#[cfg(", "test)]");
    /// 调用点个数：宽行的单行内联一处，窄行的显式两行一处。
    const CALL_SITES: usize = 2;
    /// 写守卫时实测函数体有 11 条顶层 `;` 语句。取下界而非相等：加语句不算坏，
    /// 把函数搬空才红。
    const MIN_TOP_LEVEL_STATEMENTS: usize = 11;

    /// 读源文件，并把 CRLF 归一成 LF：签出成哪种行尾取决于机器上的 git 配置，
    /// 按字节切片时不赌它。
    fn source() -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("serial.rs");
        fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("读取 {path:?} 失败：{error}"))
            .replace("\r\n", "\n")
    }

    /// 把行注释与字符串字面量的内容替换成空格，长度和换行位置都不变，之后的
    /// 花括号配对与 `;` 计数只需普通扫描。
    fn mask_comments_and_strings(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '/' if chars.peek() == Some(&'/') => {
                    out.push_str("  ");
                    for inner in chars.by_ref() {
                        if inner == '\n' {
                            out.push('\n');
                            break;
                        }
                        out.push(' ');
                    }
                }
                '"' => {
                    out.push(' ');
                    let mut escaped = false;
                    for inner in chars.by_ref() {
                        out.push(' ');
                        if inner == '\n' {
                            break; // 本文件没有跨行字符串，防御性收尾
                        }
                        if escaped {
                            escaped = false;
                        } else if inner == '\\' {
                            escaped = true;
                        } else if inner == '"' {
                            break;
                        }
                    }
                }
                other => out.push(other),
            }
        }
        out
    }

    /// 只看测试模块之前的部分：守卫不该扫到自己。
    fn production(text: &str) -> &str {
        match text.find(TEST_MODULE) {
            Some(at) => &text[..at],
            None => text,
        }
    }

    /// 取 `needle` 之后第一对花括号之间的内容（不含花括号）。传入的应当是抹过
    /// 字面量的文本，否则字符串里的括号会让配对错位。
    fn slice_braced<'t>(text: &'t str, needle: &str) -> &'t str {
        let at = text
            .find(needle)
            .unwrap_or_else(|| panic!("文件里找不到 `{needle}`：守卫要盯的东西不见了"));
        let open = text[at..]
            .find('{')
            .unwrap_or_else(|| panic!("`{needle}` 后面没有花括号"));
        let body = &text[at + open + 1..];
        let mut depth = 1usize;
        for (index, ch) in body.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &body[..index];
                    }
                }
                _ => {}
            }
        }
        panic!("`{needle}` 的花括号没有闭合");
    }

    /// 每个 `PortEditorInputs { ... }` 字面量的内部文本。
    fn input_literals(code: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        while let Some(at) = code[offset..].find(INPUTS_LITERAL) {
            let start = offset + at;
            let region = slice_braced(&code[start..], INPUTS_LITERAL);
            out.push(region);
            offset = start + INPUTS_LITERAL.len() + region.len() + 1;
        }
        out
    }

    /// 数函数体内顶层（深度 0）以 `;` 结尾的语句。
    fn top_level_statements(body: &str) -> usize {
        let mut depth = 0usize;
        let mut count = 0usize;
        for ch in body.chars() {
            match ch {
                '{' | '(' | '[' => depth += 1,
                '}' | ')' | ']' => depth = depth.saturating_sub(1),
                ';' if depth == 0 => count += 1,
                _ => {}
            }
        }
        count
    }

    /// 一段文本末尾的标识符，即 `X: &mut ...` 里的 `X`。
    fn trailing_ident(text: &str) -> Option<String> {
        let name: String = text
            .trim_end()
            .chars()
            .rev()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .collect();
        (!name.is_empty()).then(|| name.chars().rev().collect())
    }

    /// 找出区域内所有 `X: &mut Y` 且 `X != Y` 的交叉绑定，返回 `(字段, 值)`。
    fn cross_bindings(region: &str) -> Vec<(String, String)> {
        const SEP: &str = concat!(": ", "&mut ");
        let mut out = Vec::new();
        for (at, _) in region.match_indices(SEP) {
            let Some(field) = trailing_ident(&region[..at]) else {
                continue;
            };
            let value_text = region[at + SEP.len()..].trim_start_matches([' ', '&', '*']);
            let value: String = value_text
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                .collect();
            if value.is_empty() {
                continue;
            }
            // `&mut inputs.foo` 这种路径形状不归这条规则管，交给类型系统判断。
            if value_text[value.len()..].starts_with('.') {
                continue;
            }
            if value != field {
                out.push((field, value));
            }
        }
        out
    }

    #[test]
    fn deleted_group_sync_block_stays_deleted() {
        // 扫的是整个文件连注释一起。这个参数只为“未来调用者”兜底，而这种调用者
        // 并不存在：两个调用点的初值都取自同一个 `group_name`，而 egui 0.35 的
        // `selectable_value` 只在写值那一支才 `mark_changed`，所以“选中值不同于
        // 初值”等价于回调里那两个 `if … .changed()` 本帧已经跑过，块内的
        // remove/insert 是逐字重复。参数名由 `DEAD_SYNC` 在下面的消息里展开。
        let text = source();
        let found = text.matches(DEAD_SYNC).count();
        assert_eq!(
            found, 0,
            "`{DEAD_SYNC}` 在文件里出现 {found} 次，应为 0 次：它只为一个不存在的\
             调用者兜底，删掉的同步块不要回来。"
        );
    }

    #[test]
    fn port_editor_fields_bind_to_their_own_local_at_both_call_sites() {
        let text = source();
        let code = mask_comments_and_strings(production(&text));

        // 先挡住“代码被搬空、守卫还是绿”这种退化。
        let literals = code.matches(INPUTS_LITERAL).count();
        assert_eq!(
            literals, CALL_SITES,
            "结构体字面量 `{INPUTS_LITERAL}` 应有 {CALL_SITES} 个（两处调用点各一个），\
             实际 {literals} 个。"
        );
        let body = slice_braced(&code, EDITOR_FN);
        let statements = top_level_statements(body);
        assert!(
            statements >= MIN_TOP_LEVEL_STATEMENTS,
            "`render_port_editor` 只剩 {statements} 条顶层语句，少于实测的 \
             {MIN_TOP_LEVEL_STATEMENTS} 条：函数体大概被搬走了，这条守卫会退化。"
        );

        // 交叉绑定先查：写反的一对字段同时也会打破下面的计数断言，但只有这条
        // 能同时点名「哪个字段被绑到了哪个局部」，报错更快。
        for region in input_literals(&code) {
            let swapped = cross_bindings(region);
            assert!(
                swapped.is_empty(),
                "PortEditorInputs 字面量里出现交叉绑定：{}\n相邻的 `&mut String` \
                 写反是编译得过的，别名文字会进分组表、组名会进别名。每条都该写成\
                 `字段: &mut 同名的局部`。",
                swapped
                    .iter()
                    .map(|(field, value)| format!("`{field}: &mut {value}`"))
                    .collect::<Vec<_>>()
                    .join("、")
            );
        }

        // 字段名 ↔ 同名局部变量的绑定，两处调用点各一条。
        for (binding, field) in [(ALIAS_BINDING, "alias"), (GROUP_BINDING, "selected_group")] {
            let found = code.matches(binding).count();
            assert_eq!(
                found, CALL_SITES,
                "字段 `{field}` 的同名绑定 `{binding}` 应有 {CALL_SITES} 条（每处调用点\
                 一条），实际 {found} 条。少了说明有人不再绑同名局部，多了说明调用点\
                 不止两处。"
            );
        }
    }
}

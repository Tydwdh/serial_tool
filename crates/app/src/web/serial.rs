//! 浏览器串口域：端口选择与别名映射、可用端口列表的收敛，连接/断开/刷新命令的派发，
//! 以及设备拔出后的自动重连挂起与指数退避；串口面板（串口参数卡片 + 可用端口卡片）
//! 的渲染也在这里。
//!
//! 全部类型定义仍留在 `web/mod.rs`，串口状态用的 `WebReconnectState` 连它的构造函数
//! `web_reconnect_state` 一起留在那儿（构造与 `matches` 判定同处一地，这里只搬行为）。
//! 被 `web/mod.rs` 调用的项一律 `pub(crate)`，其余保持私有。

use super::{WebSerialState, web_card_header, web_now_seconds, web_reconnect_state};
use crate::workbench_app::WorkbenchApp;
use eframe::egui;
use egui_material_icons::icons::{ICON_CABLE, ICON_CANCEL, ICON_REFRESH, ICON_TUNE};
use std::collections::BTreeMap;
use tool_application::web::WebRuntime;
use tool_application::{AppCommand, CommandOutcome, TaskId, TransportView};
use tool_panels::{
    NetworkSerialAction, NetworkSerialFormView, SerialAction, SerialPanel, SerialPortItem,
    SerialPortMetadata, SerialTopBarAction, SerialView, design, network_serial_form_ui, theme,
};
use tool_platform::{
    NetworkSerialConfig, PortDescriptor, PortId, PortKind, SerialParity, SerialSettings,
};

impl WorkbenchApp {
    pub(crate) fn dispatch_serial(&mut self, command: AppCommand, ctx: &egui::Context) {
        let silent_refresh = matches!(&command, AppCommand::RefreshPorts)
            && self
                .runtime
                .as_ref()
                .is_some_and(WebRuntime::serial_supported);
        if matches!(
            &command,
            AppCommand::Connect { .. }
                | AppCommand::Disconnect { .. }
                | AppCommand::Reconnect { .. }
        ) {
            self.serial.borrow_mut().reconnect = None;
        }
        let (status, failed) = match self.runtime.as_ref() {
            Some(runtime) => match runtime.dispatch(command) {
                Ok(CommandOutcome::Pending { message, .. }) => (message, false),
                Ok(CommandOutcome::Done) => ("操作完成".to_owned(), false),
                Err(error) => (format!("操作失败：{error}"), true),
            },
            None => ("当前浏览器不支持 Web Serial".to_owned(), true),
        };
        if silent_refresh && !failed {
            self.serial.borrow_mut().status.clear();
        } else {
            self.serial.borrow_mut().status = status;
        }
        ctx.request_repaint();
    }

    pub(crate) fn select_web_port(&mut self, selected: Option<PortId>) {
        select_web_port_state(&mut self.serial.borrow_mut(), selected);
    }

    pub(crate) fn tick_web_reconnect(&mut self, ctx: &egui::Context) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        let now = web_now_seconds();
        let request = {
            let mut serial = self.serial.borrow_mut();
            if !serial.auto_reconnect {
                let cancellation = serial
                    .reconnect
                    .as_ref()
                    .map(|pending| (pending.port.clone(), pending.task_id));
                serial.reconnect = None;
                cancellation.map(|(port, task_id)| (port, task_id, None))
            } else {
                let Some(pending) = serial.reconnect.as_mut() else {
                    return;
                };
                if pending.task_id.is_some() || now < pending.next_attempt_at {
                    if pending.task_id.is_none() {
                        ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                            (pending.next_attempt_at - now).max(0.05),
                        ));
                    }
                    return;
                }
                Some((pending.port.clone(), None, Some(serial.settings)))
            }
        };

        let Some((port, task_id, settings)) = request else {
            return;
        };
        if let Some(task_id) = task_id {
            let _ = runtime.cancel_task(task_id);
            let _ = runtime.dispatch(AppCommand::CancelReconnect { port });
            return;
        }
        let Some(settings) = settings else {
            return;
        };

        match runtime.dispatch(AppCommand::Connect {
            port: port.clone(),
            settings,
        }) {
            Ok(CommandOutcome::Pending { task_id, message }) => {
                let mut serial = self.serial.borrow_mut();
                if let Some(pending) = serial
                    .reconnect
                    .as_mut()
                    .filter(|pending| pending.port == port)
                {
                    pending.task_id = Some(task_id);
                    serial.status = message;
                    ctx.request_repaint();
                }
            }
            Ok(CommandOutcome::Done) => {
                self.serial.borrow_mut().reconnect = None;
            }
            Err(error) => {
                let mut serial = self.serial.borrow_mut();
                let mut stop = false;
                if let Some(pending) = serial
                    .reconnect
                    .as_mut()
                    .filter(|pending| pending.port == port)
                {
                    pending.attempts = pending.attempts.saturating_add(1);
                    if pending.attempts >= 10 {
                        stop = true;
                    } else {
                        pending.next_attempt_at = now + web_reconnect_delay(pending.attempts);
                    }
                }
                if stop {
                    serial.reconnect = None;
                    serial.status = format!("自动重连失败（已尝试 10 次）：{error}");
                } else {
                    serial.status = format!("自动重连失败，将稍后重试：{error}");
                }
            }
        }
    }

    /// 用户取消自动重连：先丢掉待重连状态；端口已知时让运行时撤掉仍挂着的连接任务，
    /// 否则只回写一条状态文案。
    pub(crate) fn cancel_web_reconnect(&mut self, port: Option<PortId>, ctx: &egui::Context) {
        self.serial.borrow_mut().reconnect = None;
        if let Some(port) = port {
            self.dispatch_serial(AppCommand::CancelReconnect { port }, ctx);
        } else {
            self.serial.borrow_mut().status = "已取消自动重连".to_owned();
        }
    }

    fn serial_panel_ui(&mut self, ui: &mut egui::Ui, show_settings: bool, show_ports: bool) {
        let ctx = ui.ctx().clone();
        let transport_view = self.runtime.as_ref().map(WebRuntime::query_transport);
        let previous_settings = self.serial.borrow().settings;
        let mut actions = Vec::new();
        let mut network_to_add = None;
        let mut auto_reconnect_changed = false;

        // Keep the same visual order as Native: serial parameters, recording,
        // then the available-port list. The shared SerialPanel supplies the
        // individual sections; the composition root only orders them.
        if show_settings {
            let mut serial = self.serial.borrow_mut();
            design::card().show(ui, |ui| {
                web_card_header(ui, ICON_TUNE, "串口参数");
                SerialPanel::settings_ui(ui, &mut serial.settings);
                if ui
                    .checkbox(&mut serial.auto_reconnect, "串口拔出后自动重连")
                    .changed()
                {
                    auto_reconnect_changed = true;
                }
            });
            drop(serial);
            self.web_recording_ui(ui);
        }

        let (settings_changed, metadata_changed) = {
            let mut serial = self.serial.borrow_mut();
            let previous_aliases = serial.port_aliases.clone();
            let previous_groups = serial.port_groups.clone();
            let connected_port = web_connected_port(transport_view.as_ref(), &serial);
            let connecting_port = transport_view
                .as_ref()
                .filter(|view| view.connecting)
                .and_then(|_| serial.selected_port.clone());
            let pending_reconnect_port = serial
                .reconnect
                .as_ref()
                .map(|pending| pending.port.clone());
            let ports: Vec<SerialPortItem> = transport_view
                .as_ref()
                .map(|view| &view.ports)
                .unwrap_or(&serial.ports)
                .iter()
                .map(|port| SerialPortItem {
                    id: port.id.to_string(),
                    label: port.label.clone(),
                    kind: if port.kind == PortKind::Network {
                        "网络".to_owned()
                    } else {
                        String::new()
                    },
                    open: connected_port.as_ref() == Some(&port.id),
                    connecting: connecting_port.as_ref() == Some(&port.id),
                    pending_reconnect: pending_reconnect_port.as_ref() == Some(&port.id),
                })
                .collect();
            // 面板高亮与展示必须同源：直接复用上面算出的端口，只再转成字符串。
            let connected = connected_port.as_ref().map(ToString::to_string);
            let connecting = connecting_port.as_ref().map(ToString::to_string);
            let status = transport_view
                .as_ref()
                .filter(|view| !view.status.is_empty())
                .map(|view| view.status.clone())
                .unwrap_or_else(|| serial.status.clone());
            let connected_is_network = serial
                .connected
                .as_ref()
                .is_some_and(|port| port.as_str().starts_with("network://"));
            let network_port_available = ports.iter().any(|port| port.kind == "网络");
            let capabilities = if connected_is_network
                || (self
                    .runtime
                    .as_ref()
                    .is_some_and(|runtime| !runtime.serial_supported())
                    && network_port_available)
            {
                tool_platform::TransportCapabilities::WEB_NETWORK
            } else {
                transport_view
                    .as_ref()
                    .map(|view| view.capabilities)
                    .unwrap_or(tool_platform::TransportCapabilities::WEB_SERIAL)
            };
            let state = &mut *serial;
            let WebSerialState {
                settings,
                send_input,
                tx_hex,
                dtr,
                rts,
                port_aliases,
                port_groups,
                ..
            } = state;
            let mut view = SerialView {
                ports: &ports,
                connected: connected.as_deref(),
                connecting: connecting.as_deref(),
                status: &status,
                settings,
                send_input,
                tx_hex,
                dtr,
                rts,
                capabilities,
                show_ports,
                show_sender: false,
                metadata: show_settings.then_some(SerialPortMetadata {
                    aliases: port_aliases,
                    groups: port_groups,
                }),
            };
            if show_settings {
                design::card().show(ui, |ui| {
                    web_card_header(ui, ICON_CABLE, "可用端口");
                    // Keep the same order as Native: the network endpoint is
                    // added from the available-port card, then all endpoints
                    // (serial and network) use the same grouped list below.
                    let network_action = {
                        let mut form = NetworkSerialFormView {
                            host: &mut state.network_host,
                            port: &mut state.network_port,
                            api_key: &mut state.network_api_key,
                        };
                        network_serial_form_ui(ui, &mut form).into_iter().next()
                    };
                    match network_action {
                        Some(NetworkSerialAction::Submit(config)) => {
                            if state
                                .network_ports
                                .iter()
                                .any(|item| item.port_id() == config.port_id())
                            {
                                state.status = "网络串口已存在，正在连接".to_owned();
                            }
                            network_to_add = Some(config);
                        }
                        Some(NetworkSerialAction::Error(error)) => {
                            state.status = error;
                        }
                        None => {}
                    }
                    ui.separator();
                    actions.extend(SerialPanel::port_list_ui(ui, &mut view));
                });
            }
            (
                *settings != previous_settings,
                serial.port_aliases != previous_aliases || serial.port_groups != previous_groups,
            )
        };

        if let Some(config) = network_to_add {
            self.add_web_network_port(config, &ctx);
        }

        if settings_changed || auto_reconnect_changed || metadata_changed {
            if settings_changed {
                let settings = self.serial.borrow().settings;
                if let Some(runtime) = self.runtime.as_ref()
                    && let Err(error) = runtime.dispatch(AppCommand::SetSerialSettings { settings })
                {
                    self.serial.borrow_mut().status = format!("串口参数更新失败：{error}");
                }
                let mut serial = self.serial.borrow_mut();
                if let Some(port) = serial.selected_port.clone() {
                    serial.port_profiles.insert(port.to_string(), settings);
                }
            }
            self.persist_settings();
        }
        for action in actions {
            self.dispatch_serial_action(action, &ctx);
        }
    }

    /// 登记网络串口并立刻连接：已登记过的端点不再重复写入 `network_ports`。
    fn add_web_network_port(&mut self, config: NetworkSerialConfig, ctx: &egui::Context) {
        let port = config.port_id();
        {
            let mut serial = self.serial.borrow_mut();
            if !serial
                .network_ports
                .iter()
                .any(|item| item.port_id() == port)
            {
                serial.network_ports.push(config.clone());
            }
        }
        self.select_web_port(Some(port.clone()));
        if let Some(runtime) = self.runtime.as_ref() {
            let _ = runtime.dispatch(AppCommand::RegisterNetworkPort {
                config: config.clone(),
            });
            let settings = self.serial.borrow().settings;
            self.dispatch_serial(AppCommand::Connect { port, settings }, ctx);
        }
        self.persist_settings();
    }

    /// 串口面板的一条动作落到命令上；连接与移除网络串口还会就地更新浏览器状态。
    fn dispatch_serial_action(&mut self, action: SerialAction, ctx: &egui::Context) {
        let command = match action {
            SerialAction::Refresh => AppCommand::RefreshPorts,
            SerialAction::RequestPort => AppCommand::RequestPort,
            SerialAction::Connect { port, .. } => {
                let port = PortId::new(port);
                self.select_web_port(Some(port.clone()));
                let settings = self.serial.borrow().settings;
                AppCommand::Connect { port, settings }
            }
            SerialAction::Disconnect { port } => AppCommand::Disconnect {
                port: PortId::new(port),
            },
            SerialAction::SendText { port, text } => AppCommand::SendText {
                port: PortId::new(port),
                text,
            },
            SerialAction::SendHex { port, hex } => AppCommand::SendHex {
                port: PortId::new(port),
                hex,
                strict: self.serial.borrow().hex_strict,
            },
            SerialAction::SetDtr { port, value } => AppCommand::SetDtr {
                port: PortId::new(port),
                value,
            },
            SerialAction::SetRts { port, value } => AppCommand::SetRts {
                port: PortId::new(port),
                value,
            },
            SerialAction::CancelReconnect { port } => AppCommand::CancelReconnect {
                port: PortId::new(port),
            },
            SerialAction::RemoveNetwork { port } => {
                let port = PortId::new(port);
                {
                    let mut serial = self.serial.borrow_mut();
                    serial
                        .network_ports
                        .retain(|config| config.port_id() != port);
                    if serial.selected_port.as_ref() == Some(&port) {
                        serial.selected_port = None;
                    }
                }
                self.persist_settings();
                AppCommand::RemoveNetworkPort { port }
            }
        };
        self.dispatch_serial(command, ctx);
    }

    pub(crate) fn serial_ui(&mut self, ui: &mut egui::Ui) {
        let refresh_on_entry = {
            let mut serial = self.serial.borrow_mut();
            std::mem::take(&mut serial.refresh_on_page_entry)
        };
        if refresh_on_entry
            && self
                .runtime
                .as_ref()
                .is_some_and(WebRuntime::serial_supported)
        {
            self.dispatch_serial(AppCommand::RefreshPorts, ui.ctx());
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.serial_panel_ui(ui, true, true);
            });
    }
}

fn web_reconnect_delay(attempts: u32) -> f64 {
    (0.5 * 2_f64.powi(attempts.min(4) as i32)).min(8.0)
}

pub(crate) fn select_web_port_state(serial: &mut WebSerialState, selected: Option<PortId>) {
    if serial.selected_port == selected {
        return;
    }
    if let Some(previous) = serial.selected_port.clone() {
        serial
            .port_profiles
            .insert(previous.to_string(), serial.settings);
    }
    serial.selected_port = selected.clone();
    if let Some(port) = selected
        && let Some(settings) = serial.port_profiles.get(port.as_str()).copied()
    {
        serial.settings = settings;
    }
}

/// 端口刚被浏览器授权：先去重再选中并追加到列表末尾，保证同一端口不会在列表里出现两次。
pub(crate) fn select_authorized_port(serial: &mut WebSerialState, port: PortDescriptor) {
    serial.ports.retain(|item| item.id != port.id);
    select_web_port_state(serial, Some(port.id.clone()));
    serial.ports.push(port);
}

/// 当前已打开的端口：以 Application 的传输视图为准，视图缺位（运行时还没建好）时
/// 才回退到 composition root 的本地镜像，二者正常路径下始终一致。
pub(crate) fn web_connected_port(
    view: Option<&TransportView>,
    serial: &WebSerialState,
) -> Option<PortId> {
    view.and_then(|view| view.connected.clone())
        .or_else(|| serial.connected.clone())
}

/// 端口刷新：先让上一帧的瞬时状态退场，再按新列表收敛选中项。
pub(crate) fn apply_web_refreshed_ports(serial: &mut WebSerialState, ports: Vec<PortDescriptor>) {
    serial.status.clear();
    serial.ports = ports;
    if serial
        .selected_port
        .as_ref()
        .is_some_and(|selected| !serial.ports.iter().any(|port| &port.id == selected))
    {
        select_web_port_state(serial, None);
    }
}

/// 设备重新插上：同 id 的旧条目让位给最新描述符；待重连记录若指向同一台物理设备，
/// 跟着改名并把下一次尝试提前到立刻。
pub(crate) fn accept_web_attached_port(serial: &mut WebSerialState, port: PortDescriptor) {
    serial.ports.retain(|item| item.id != port.id);
    if serial
        .reconnect
        .as_ref()
        .is_some_and(|pending| pending.matches(&port))
        && let Some(pending) = serial.reconnect.as_mut()
    {
        pending.port = port.id.clone();
        pending.vendor_id = port.vendor_id;
        pending.product_id = port.product_id;
        pending.next_attempt_at = 0.0;
    }
    serial.ports.push(port);
    serial.status = "检测到已授权设备".to_owned();
}

/// 设备拔出：先记住拔出前的描述符，自动重连才能在端口改名后仍认出同一台设备。
pub(crate) fn forget_web_detached_port(serial: &mut WebSerialState, port: PortId) {
    let descriptor = serial.ports.iter().find(|item| item.id == port).cloned();
    serial.ports.retain(|item| item.id != port);
    if serial.connected.as_ref() == Some(&port) {
        serial.connected = None;
        serial.status = "设备已拔出".to_owned();
        if serial.auto_reconnect {
            serial.reconnect = Some(web_reconnect_state(port.clone(), descriptor));
        }
    }
    if serial.selected_port.as_ref() == Some(&port) {
        select_web_port_state(serial, None);
    }
}

/// 网络端点移除：列表、选中与连接三处状态一起收，最后落一条状态文案。
pub(crate) fn forget_web_network_port(serial: &mut WebSerialState, port: PortId) {
    serial.ports.retain(|item| item.id != port);
    if serial.selected_port.as_ref() == Some(&port) {
        select_web_port_state(serial, None);
    }
    if serial.connected.as_ref() == Some(&port) {
        serial.connected = None;
    }
    serial.status = "网络串口已移除".to_owned();
}

/// 挂起的自动重连任务失败：记一次尝试并安排下一次，满 10 次就放弃。
/// 返回 false 表示这条失败与重连无关，由调用方回落到通用文案。
pub(crate) fn handle_web_reconnect_task_failed(
    serial: &mut WebSerialState,
    task_id: TaskId,
    error: &str,
) -> bool {
    let owns_this_task = serial
        .reconnect
        .as_ref()
        .is_some_and(|pending| pending.task_id == Some(task_id));
    if !owns_this_task {
        return false;
    }
    let mut stop = false;
    if let Some(pending) = serial.reconnect.as_mut() {
        pending.task_id = None;
        pending.attempts = pending.attempts.saturating_add(1);
        if pending.attempts >= 10 {
            stop = true;
        } else {
            pending.next_attempt_at = web_now_seconds() + web_reconnect_delay(pending.attempts);
        }
    }
    if stop {
        serial.reconnect = None;
        serial.status = format!("自动重连失败（已尝试 10 次）：{error}");
    } else {
        serial.status = format!("自动重连失败，将稍后重试：{error}");
    }
    true
}

/// 挂起的自动重连任务被取消：同样记一次尝试并稍后再试。
/// 返回 false 表示这条取消与重连无关，由调用方回落到通用文案。
pub(crate) fn handle_web_reconnect_task_cancelled(
    serial: &mut WebSerialState,
    task_id: TaskId,
) -> bool {
    let Some(pending) = serial
        .reconnect
        .as_mut()
        .filter(|pending| pending.task_id == Some(task_id))
    else {
        return false;
    };
    pending.task_id = None;
    pending.attempts = pending.attempts.saturating_add(1);
    pending.next_attempt_at = web_now_seconds() + web_reconnect_delay(pending.attempts);
    serial.status = "自动重连任务已取消，将稍后重试".to_owned();
    true
}

/// 顶栏端口条目的三个标记：`open` 来自运行时看到的已连接端口，`connecting`
/// 只在「正在连接的就是这一项」时点亮，`pending_reconnect` 来自待重连的端口名。
pub(crate) fn web_top_bar_port_items(
    ports: &[PortDescriptor],
    connected_id: Option<&str>,
    connecting: bool,
    selected: Option<&PortId>,
    pending_reconnect: Option<&str>,
) -> Vec<SerialPortItem> {
    ports
        .iter()
        .map(|port| SerialPortItem {
            id: port.id.to_string(),
            label: port.label.clone(),
            kind: match port.kind {
                PortKind::Serial => "Serial",
                PortKind::Network => "网络",
                PortKind::Unknown => "",
            }
            .to_owned(),
            open: connected_id == Some(port.id.as_str()),
            connecting: connecting && selected == Some(&port.id),
            pending_reconnect: pending_reconnect == Some(port.id.as_str()),
        })
        .collect()
}

/// 顶栏的自动重连小票：显示距下一次尝试的剩余秒数与「第 N 次/10」，
/// 返回用户是否点击了取消。
pub(crate) fn web_reconnect_chip_ui(
    ui: &mut egui::Ui,
    port: &str,
    attempts: u32,
    next_attempt_at: f64,
) -> bool {
    let remaining = (next_attempt_at - web_now_seconds()).max(0.0);
    let label = format!(
        "{} 重连中 {} {:.1}s ({}/{})",
        ICON_REFRESH.codepoint,
        port,
        remaining,
        attempts + 1,
        10
    );
    ui.label(egui::RichText::new(label).color(theme::yellow()))
        .on_hover_text("点击取消自动重连");
    design::icon_button(ui, ICON_CANCEL, "取消重连").clicked()
}

/// 顶栏串口动作落到命令上；`Connect` 用的是这一帧看到的串口参数快照。
pub(crate) fn web_top_bar_serial_command(
    action: SerialTopBarAction,
    settings: SerialSettings,
) -> AppCommand {
    match action {
        SerialTopBarAction::Refresh => AppCommand::RefreshPorts,
        SerialTopBarAction::RequestPort => AppCommand::RequestPort,
        SerialTopBarAction::Connect { port } => AppCommand::Connect {
            port: PortId::new(port),
            settings,
        },
        SerialTopBarAction::Disconnect { port } => AppCommand::Disconnect {
            port: PortId::new(port),
        },
        SerialTopBarAction::Reconnect { port } => AppCommand::Reconnect {
            port: PortId::new(port),
        },
    }
}

/// 状态栏串口标签的三态：连接中 > 已连接（带串口参数）> 只显示端口名。
pub(crate) fn web_serial_status_label(
    port_label: &str,
    connecting: bool,
    connected: bool,
    settings: SerialSettings,
) -> String {
    if connecting {
        format!("{port_label} 连接中")
    } else if connected {
        format!("{port_label} @ {}", settings_label(settings))
    } else {
        port_label.to_owned()
    }
}

pub(crate) fn web_port_display_name(
    port: &PortDescriptor,
    aliases: &BTreeMap<String, String>,
) -> String {
    let alias = aliases
        .get(port.id.as_str())
        .map(|alias| alias.trim())
        .filter(|alias| !alias.is_empty());
    match alias {
        Some(alias) => format!("{alias} ({})", port.id),
        None if port.label.trim().is_empty() => port.id.to_string(),
        None => format!("{} ({})", port.label, port.id),
    }
}

pub(crate) fn settings_label(settings: SerialSettings) -> String {
    format!(
        "{} {}{}{}",
        settings.baud_rate,
        settings.data_bits,
        match settings.parity {
            SerialParity::None => 'N',
            SerialParity::Odd => 'O',
            SerialParity::Even => 'E',
        },
        settings.stop_bits
    )
}

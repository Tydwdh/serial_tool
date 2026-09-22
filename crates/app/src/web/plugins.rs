//! 浏览器插件域：Web 插件的安装与卸载、Lua 实例生命周期、marketplace 安装，
//! 以及贡献点（top_bar / status_bar / send.toolbar）的渲染与命令派发。
//!
//! 类型定义与 `WebPluginState` 的投影逻辑仍留在 `web/mod.rs`；这里只搬行为。
//! 被 `web/mod.rs` 或其他兄弟模块调用的项一律 `pub(crate)`，其余保持私有。

use super::{
    WEB_PLUGIN_API_VERSION, WebPluginManifest, WebPluginPersisted, WebPluginRecord,
    WebPluginSetting, web_connected_port, web_file_input, web_file_input_from_event,
    web_open_file_picker,
};
use crate::web_plugin_host::{WebPluginData, WebPluginHost, event_to_plugin_value};
use crate::workbench_app::WorkbenchApp;
use eframe::egui;
use egui_material_icons::icons::ICON_APPS;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use tool_application::plugin::{
    PluginCommand, PluginSettingView, PluginStateView, PluginUiContributionView, PluginView,
};
use tool_application::web::WebRuntime;
use tool_application::{AppCommand, CommandOutcome, TaskId};
use tool_core::topics;
use tool_core::{Direction, Event, Payload};
use tool_panels::{
    PluginPanelEvent, PluginPanelOptions, PluginSettingsView, design, plugin_settings_ui, theme,
};
use tool_platform::PortKind;
use tool_plugin_api::{
    LuaEngine, PluginCallResult, PluginHostApi, PluginHostCompletion, PluginHostPendingRequest,
    PluginLoadConfig, PluginPermissions, PluginSerialDevice, PluginValue,
};
use wasm_bindgen_futures::JsFuture;

impl WorkbenchApp {
    pub(crate) fn sync_web_plugin_view(&self) {
        let Some(runtime) = self.runtime.as_ref() else {
            return;
        };
        runtime.set_plugins_view(PluginView {
            summaries: self.plugins.summaries(),
            diagnostics: Vec::new(),
        });
    }

    fn request_web_plugin_files(&mut self, _ctx: &egui::Context) {
        let Some(document) = web_sys::window().and_then(|window| window.document()) else {
            return;
        };
        let Some(input) = web_file_input(&document) else {
            return;
        };
        input.set_multiple(true);
        input.set_accept(".json,.lua");
        let Some(runtime) = self.runtime.clone() else {
            self.serial.borrow_mut().status = "当前浏览器没有可用的异步任务运行时".to_owned();
            return;
        };
        web_open_file_picker(&input, move |event: web_sys::Event| {
            let Some(input) = web_file_input_from_event(&event) else {
                return;
            };
            let Some(files) = input.files() else {
                return;
            };
            let mut files_to_read = Vec::new();
            for index in 0..files.length() {
                if let Some(file) = files.get(index) {
                    files_to_read.push((file.name(), file));
                }
            }
            let future = async move {
                let mut contents = Vec::with_capacity(files_to_read.len());
                for (name, file) in files_to_read {
                    let text = JsFuture::from(file.text())
                        .await
                        .map_err(|error| format!("读取插件文件失败：{error:?}"))?
                        .as_string()
                        .ok_or_else(|| format!("插件文件不是文本：{name}"))?;
                    contents.push((name, text));
                }
                Ok::<_, String>(contents)
            };
            let _ = runtime.load_files("plugin_import", future);
        });
    }

    pub(crate) fn load_web_plugin(&mut self, index: usize) {
        let runtime = self.runtime.clone();
        let Some(record) = self.plugins.records.get(index) else {
            return;
        };
        if record.loading || record.lua_instance.is_some() || record.replay_instance.is_some() {
            return;
        }
        let plugin_name = record.persisted.manifest.name.clone();
        let plugin_id = record.persisted.manifest.id.clone();
        let plugin_version = record.persisted.manifest.version.clone();
        let script_name = record.persisted.manifest.live_main().to_owned();
        let runtime_name = record.persisted.manifest.runtime.clone();
        let source = record.persisted.source.clone();
        let replay_source = record.persisted.replay_source.clone();
        let replay_manifest = record.persisted.manifest.replay.clone();
        let settings =
            serde_json::to_string(&record.persisted.settings).unwrap_or_else(|_| "{}".to_owned());
        let permissions = record.persisted.manifest.live_permissions().to_vec();
        let context = PluginValue::from_json(&serde_json::json!({
            "settings": record.persisted.settings,
            "plugin_id": plugin_id,
        }));
        let Some(record) = self.plugins.records.get_mut(index) else {
            return;
        };
        record.loading = true;
        record.error = None;
        record.load_task = runtime.as_ref().map(|runtime| {
            runtime.begin_task("plugin_load", format!("正在加载 Lua 插件：{plugin_name}"))
        });

        if runtime_name != "lua" {
            let error = format!(
                "插件 {} 使用 runtime={}；Native/Web 统一使用 runtime=lua + main.lua",
                plugin_id, runtime_name
            );
            record.loading = false;
            record.error = Some(error.clone());
            if let Some(task_id) = record.load_task.take()
                && let Some(runtime) = runtime.as_ref()
            {
                runtime.fail_task(task_id, error);
            }
            return;
        }

        {
            let Some(runtime) = runtime else {
                record.loading = false;
                record.error = Some("Web Application runtime 不可用".to_owned());
                return;
            };
            self.plugin_data
                .borrow_mut()
                .entry(plugin_id.clone())
                .or_insert_with(|| WebPluginData {
                    settings: serde_json::from_str(&settings).unwrap_or_default(),
                    storage: BTreeMap::new(),
                    profiles: BTreeMap::new(),
                });
            let permission_set = PluginPermissions::from_permission_names(permissions);
            if let Some(error) = web_missing_capability_error(&permission_set) {
                record.loading = false;
                record.error = Some(error.clone());
                if let Some(task_id) = record.load_task.take() {
                    runtime.fail_task(task_id, error);
                }
                return;
            }
            let host = Rc::new(WebPluginHost::new(
                runtime.clone(),
                plugin_id.clone(),
                self.plugin_data.clone(),
            ));
            let config = PluginLoadConfig {
                plugin_id: plugin_id.clone(),
                plugin_name: plugin_name.clone(),
                plugin_version: plugin_version.clone(),
                script_name,
                context: context.clone(),
                permissions: permission_set.clone(),
            };
            record.host = Some(host.clone());
            match self.web_lua.load_plugin(&source, config, host) {
                Ok(instance) => {
                    record.lua_instance = Some(instance);
                    if let (Some(replay_source), Some(replay_manifest)) =
                        (replay_source.as_deref(), replay_manifest.as_ref())
                    {
                        let replay_config = PluginLoadConfig {
                            plugin_id: plugin_id.clone(),
                            plugin_name: plugin_name.clone(),
                            plugin_version: plugin_version.clone(),
                            script_name: replay_manifest.main.clone(),
                            context,
                            permissions: permission_set,
                        };
                        match self.web_lua.load_replay_plugin(
                            replay_source,
                            replay_config,
                            replay_manifest.outputs.clone(),
                            record.host.as_ref().expect("host installed").clone(),
                        ) {
                            Ok(replay_instance) => record.replay_instance = Some(replay_instance),
                            Err(error) => {
                                record.error = Some(format!("Replay Lua 加载失败：{error}"));
                            }
                        }
                    }
                    record.loading = false;
                    record.panels_published = true;
                    if let Some(task_id) = record.load_task.take() {
                        runtime.complete_task(task_id, "Lua 插件已加载");
                    }
                    self.publish_web_plugin_panels(index);
                }
                Err(error) => {
                    record.loading = false;
                    record.error = Some(error.to_string());
                    if let Some(task_id) = record.load_task.take() {
                        runtime.fail_task(task_id, error.to_string());
                    }
                }
            }
        }
    }

    fn publish_web_plugin_panels(&self, index: usize) {
        let Some(runtime) = &self.runtime else {
            return;
        };
        let Some(record) = self.plugins.records.get(index) else {
            return;
        };
        if !record.persisted.manifest.has_permission("ui") {
            return;
        }
        let source = format!("plugin:{}", record.persisted.manifest.id);
        for panel in record.persisted.manifest.contributes.panels.clone() {
            runtime.publish_event(Event::new(
                topics::UI_PANEL_CREATE,
                source.clone(),
                Direction::Internal,
                Payload::Json(panel),
            ));
        }
    }

    fn remove_web_plugin_panels(&self, index: usize) {
        let Some(runtime) = &self.runtime else {
            return;
        };
        let Some(record) = self.plugins.records.get(index) else {
            return;
        };
        let source = format!("plugin:{}", record.persisted.manifest.id);
        for panel in &record.persisted.manifest.contributes.panels {
            let Some(id) = panel.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            runtime.publish_event(Event::new(
                topics::UI_PANEL_REMOVE,
                source.clone(),
                Direction::Internal,
                Payload::Json(serde_json::json!({ "id": id })),
            ));
        }
    }

    /// 按清单 id 定位插件记录。浏览器插件以 manifest.id 为唯一键，安装顺序只是展示顺序。
    fn web_plugin_index(&self, plugin_id: &str) -> Option<usize> {
        self.plugins
            .records
            .iter()
            .position(|record| record.persisted.manifest.id == plugin_id)
    }

    fn install_web_plugin(&mut self, persisted: WebPluginPersisted) {
        let existing = self.web_plugin_index(&persisted.manifest.id);
        let index = if let Some(index) = existing {
            self.unload_web_plugin(index);
            self.plugins.records[index] = WebPluginRecord::new(persisted);
            index
        } else {
            self.plugins.records.push(WebPluginRecord::new(persisted));
            self.plugins.records.len() - 1
        };
        if self.plugins.records[index].persisted.enabled {
            self.load_web_plugin(index);
        }
        self.persist_settings();
    }

    fn uninstall_web_plugin(&mut self, plugin_id: &str) {
        let Some(index) = self.web_plugin_index(plugin_id) else {
            self.serial.borrow_mut().status = format!("Web 插件不存在：{plugin_id}");
            return;
        };
        self.unload_web_plugin(index);
        self.plugins.records.remove(index);
        if !self
            .plugins
            .records
            .iter()
            .any(|record| record.persisted.enabled)
            && let Some(runtime) = self.runtime.as_ref()
        {
            runtime.clear_plugin_events();
        }
        self.persist_settings();
        self.serial.borrow_mut().status = format!("Web 插件已卸载：{plugin_id}");
    }

    pub(crate) fn apply_web_plugin_files(&mut self, files: Vec<(String, String)>) {
        match parse_web_plugin_files(&files) {
            Ok(persisted) => self.install_web_plugin(persisted),
            Err(error) => self.serial.borrow_mut().status = error,
        }
    }

    pub(crate) fn apply_web_marketplace_files(
        &mut self,
        plugin_id: String,
        files: Vec<(String, String)>,
    ) {
        let entry = self
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.query_marketplace().registry)
            .and_then(|registry| {
                registry
                    .plugins
                    .into_iter()
                    .find(|entry| entry.id == plugin_id)
            });
        match parse_web_plugin_files(&files) {
            Ok(persisted) => {
                self.install_web_plugin(persisted);
                if let Some(runtime) = self.runtime.as_ref() {
                    runtime.finish_marketplace_install(&plugin_id, Ok(()));
                }
                if let Some(entry) = entry {
                    self.serial.borrow_mut().status =
                        format!("已安装 Web 插件 {} v{}", entry.name, entry.version);
                }
            }
            Err(error) => {
                if let Some(runtime) = self.runtime.as_ref() {
                    runtime.finish_marketplace_install(
                        &plugin_id,
                        Err(format!("安装 Web 插件失败：{error}")),
                    );
                }
            }
        }
    }

    pub(crate) fn poll_web_plugins(&mut self, _ctx: &egui::Context) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        let plugin_commands = runtime.take_plugin_commands();
        for command in plugin_commands {
            match command {
                PluginCommand::Enable { plugin_id } => {
                    self.set_web_plugin_enabled(&plugin_id, true);
                }
                PluginCommand::Disable { plugin_id } => {
                    self.set_web_plugin_enabled(&plugin_id, false);
                }
                PluginCommand::Reload => {
                    let enabled = self
                        .plugins
                        .records
                        .iter()
                        .enumerate()
                        .filter_map(|(index, record)| record.persisted.enabled.then_some(index))
                        .collect::<Vec<_>>();
                    for index in enabled {
                        self.unload_web_plugin(index);
                        self.load_web_plugin(index);
                    }
                }
                PluginCommand::Execute {
                    plugin_id,
                    command_id,
                    context,
                } => {
                    self.execute_web_plugin_command(plugin_id, command_id, context);
                }
            }
        }
        self.sync_web_plugin_view();
        let events = if self
            .plugins
            .records
            .iter()
            .any(|record| record.persisted.enabled)
        {
            runtime.drain_plugin_events(256)
        } else {
            runtime.clear_plugin_events();
            Vec::new()
        };
        for event in events {
            let event_value = event_to_plugin_value(&event);
            for record in &self.plugins.records {
                if !record.persisted.enabled {
                    continue;
                }
                let manifest = &record.persisted.manifest;
                if !manifest.has_permission("bus") || !manifest.receives_event(&event) {
                    continue;
                }
                let Some(instance) = record.lua_instance else {
                    continue;
                };
                if let Err(error) = self.web_lua.dispatch_event(instance, event_value.clone()) {
                    self.serial.borrow_mut().status = format!("Lua 插件发送事件失败：{error}");
                }
            }
        }
    }

    fn request_web_marketplace_refresh(&mut self, _ctx: &egui::Context) {
        let url = self.marketplace_url.clone();
        self.dispatch_command_to_status(AppCommand::RefreshMarketplace { url });
    }

    fn install_web_marketplace_entry(
        &mut self,
        plugin_id: String,
        manifest_url: String,
        main_url: String,
    ) {
        let Some(runtime) = self.runtime.as_ref() else {
            self.serial.borrow_mut().status = "异步任务运行时不可用".to_owned();
            return;
        };
        if let Err(error) = runtime.dispatch(AppCommand::InstallMarketplacePlugin {
            plugin_id,
            manifest_url,
            main_url,
        }) {
            self.serial.borrow_mut().status = error;
        }
    }

    /// 启用/禁用插件：只有状态真的翻转时才加载或卸载并落盘，避免重复触发 Lua 生命周期。
    fn set_web_plugin_enabled(&mut self, plugin_id: &str, enabled: bool) {
        let Some(index) = self.web_plugin_index(plugin_id) else {
            return;
        };
        if self.plugins.records[index].persisted.enabled == enabled {
            return;
        }
        self.plugins.records[index].persisted.enabled = enabled;
        if enabled {
            self.load_web_plugin(index);
        } else {
            self.unload_web_plugin(index);
        }
        self.persist_settings();
    }

    /// 作废某个插件留下的挂起 Lua 能力请求：只保留宿主句柄（Rc 身份）不同的项。
    fn drop_pending_lua_requests(
        pending: &RefCell<BTreeMap<TaskId, (Rc<WebPluginHost>, String)>>,
        owner: Option<&Rc<WebPluginHost>>,
    ) {
        pending
            .borrow_mut()
            .retain(|_, (host, _)| owner.is_none_or(|owner| !Rc::ptr_eq(owner, host)));
    }

    fn unload_web_plugin(&mut self, index: usize) {
        self.remove_web_plugin_panels(index);
        if let Some(plugin_id) = self
            .plugins
            .records
            .get(index)
            .map(|record| record.persisted.manifest.id.clone())
        {
            self.plugins.clear_contribution_values(&plugin_id);
        }
        let owner = self
            .plugins
            .records
            .get(index)
            .and_then(|record| record.host.as_ref());
        Self::drop_pending_lua_requests(&self.plugins.pending_lua_file_requests, owner);
        Self::drop_pending_lua_requests(&self.plugins.pending_lua_serial_requests, owner);
        let load_task = self
            .plugins
            .records
            .get_mut(index)
            .and_then(|record| record.load_task.take());
        if let Some(task_id) = load_task
            && let Some(runtime) = self.runtime.as_ref()
        {
            runtime.cancel_task(task_id);
        }
        let (lua_instance, replay_instance) = self
            .plugins
            .records
            .get(index)
            .map(|record| (record.lua_instance, record.replay_instance))
            .unwrap_or((None, None));
        for instance in [lua_instance, replay_instance].into_iter().flatten() {
            let _ = self.web_lua.stop(instance);
        }
        let Some(record) = self.plugins.records.get_mut(index) else {
            return;
        };
        record.lua_instance = None;
        record.replay_instance = None;
        record.host = None;
        record.loading = false;
    }

    pub(crate) fn resolve_web_lua_file_request(&mut self, task_id: TaskId, ok: bool, result: &str) {
        let Some((host, request_id)) = self
            .plugins
            .pending_lua_file_requests
            .borrow_mut()
            .remove(&task_id)
        else {
            return;
        };
        let completion = PluginHostCompletion {
            request_id,
            result: if ok {
                Ok(PluginValue::String(result.to_owned()))
            } else {
                Err(result.to_owned())
            },
        };
        if let Err(error) = host.complete_request(completion) {
            self.serial.borrow_mut().status = format!("Lua 文件请求失败：{error}");
        }
    }

    pub(crate) fn resolve_web_lua_serial_request(
        &mut self,
        task_id: TaskId,
        result: Result<PluginSerialDevice, String>,
    ) {
        let Some((host, request_id)) = self
            .plugins
            .pending_lua_serial_requests
            .borrow_mut()
            .remove(&task_id)
        else {
            return;
        };
        let completion = PluginHostCompletion {
            request_id,
            result: result.map(|device| {
                PluginValue::from_json(
                    &serde_json::to_value(device).unwrap_or(serde_json::Value::Null),
                )
            }),
        };
        if let Err(error) = host.complete_request(completion) {
            self.serial.borrow_mut().status = format!("Lua 串口授权请求失败：{error}");
        }
    }

    pub(crate) fn poll_web_lua_host_requests(&mut self) {
        let hosts = self
            .plugins
            .records
            .iter()
            .filter_map(|record| record.host.clone())
            .collect::<Vec<_>>();
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        for host in hosts {
            for request in host.take_pending_requests() {
                if let PluginHostPendingRequest::SerialRequestDevice { request_id, .. } = request {
                    match runtime.dispatch(AppCommand::RequestPort) {
                        Ok(CommandOutcome::Pending { task_id, .. }) => {
                            self.plugins
                                .pending_lua_serial_requests
                                .borrow_mut()
                                .insert(task_id, (host.clone(), request_id));
                        }
                        Ok(CommandOutcome::Done) => {
                            fail_lua_host_request(&host, &request_id, "浏览器串口授权请求未启动");
                        }
                        Err(error) => fail_lua_host_request(&host, &request_id, &error),
                    }
                    continue;
                }
                let PluginHostPendingRequest::FileOpenText {
                    request_id,
                    title,
                    extensions,
                    ..
                } = request
                else {
                    continue;
                };
                let Some(document) = web_sys::window().and_then(|window| window.document()) else {
                    fail_lua_host_request(&host, &request_id, "浏览器文件选择器不可用");
                    continue;
                };
                let Some(input) = web_file_input(&document) else {
                    fail_lua_host_request(&host, &request_id, "创建文件选择器失败");
                    continue;
                };
                let accept = extensions
                    .iter()
                    .filter(|extension| extension.as_str() != "*")
                    .map(|extension| {
                        if extension.starts_with('.') {
                            extension.clone()
                        } else {
                            format!(".{extension}")
                        }
                    })
                    .collect::<Vec<_>>();
                if !accept.is_empty() {
                    input.set_accept(&accept.join(","));
                }
                let host_for_change = host.clone();
                let request_id_for_change = request_id.clone();
                let pending = self.plugins.pending_lua_file_requests.clone();
                let runtime_for_change = runtime.clone();
                let title_for_task = title.clone();
                let opened =
                    web_open_file_picker(&input, move |event: web_sys::Event| {
                        let Some(input) = web_file_input_from_event(&event) else {
                            return;
                        };
                        let Some(file) = input.files().and_then(|files| files.get(0)) else {
                            fail_lua_host_request(
                                &host_for_change,
                                &request_id_for_change,
                                "未选择文件",
                            );
                            return;
                        };
                        let future = async move {
                            JsFuture::from(file.text())
                                .await
                                .map_err(|error| format!("读取文件失败：{error:?}"))
                                .and_then(|value| {
                                    value
                                        .as_string()
                                        .ok_or_else(|| "文件不是有效文本".to_owned())
                                })
                        };
                        if let CommandOutcome::Pending { task_id, .. } = runtime_for_change
                            .load_text("lua_host_file", title_for_task.clone(), future)
                        {
                            pending.borrow_mut().insert(
                                task_id,
                                (host_for_change.clone(), request_id_for_change.clone()),
                            );
                        }
                    });
                if !opened {
                    fail_lua_host_request(&host, &request_id, "打开文件选择器失败");
                }
            }
        }
    }

    pub(crate) fn web_plugins_ui(&mut self, ui: &mut egui::Ui) {
        let Some(runtime) = self.runtime.clone() else {
            ui.label("当前浏览器没有可用的 Application runtime");
            return;
        };
        let plugin_view = runtime.query_plugins();
        let marketplace_view = runtime.query_marketplace();

        if let Some(registry) = marketplace_view.registry.clone() {
            self.plugins_panel
                .set_market_registry_view(registry, self.marketplace_url.clone());
        } else {
            self.plugins_panel.clear_market_registry();
        }
        self.plugins_panel
            .set_market_refreshing(marketplace_view.refreshing);
        if let Some(error) = marketplace_view.error {
            self.plugins_panel.set_market_error(error);
        }
        self.plugins_panel.set_installed_ids(
            plugin_view
                .summaries
                .iter()
                .map(|summary| summary.id.clone())
                .collect(),
        );
        if let Some(registry) = marketplace_view.registry.as_ref() {
            for entry in &registry.plugins {
                if marketplace_view.installing.iter().any(|id| id == &entry.id) {
                    self.plugins_panel.mark_installing(&entry.id);
                } else {
                    self.plugins_panel.clear_installing(&entry.id);
                }
            }
        }

        let events = self.plugins_panel.ui_with_view_options(
            ui,
            &plugin_view.summaries,
            &plugin_view.diagnostics,
            PluginPanelOptions {
                show_root: false,
                show_import: true,
                marketplace_url: Some(&mut self.marketplace_url),
            },
        );
        for event in events {
            match event {
                PluginPanelEvent::Status(message, _is_error) => {
                    self.serial.borrow_mut().status = message;
                }
                PluginPanelEvent::Enable(plugin_id) => {
                    if let Err(error) = runtime.dispatch(AppCommand::EnablePlugin { plugin_id }) {
                        self.serial.borrow_mut().status = error;
                    }
                }
                PluginPanelEvent::Disable(plugin_id) => {
                    if let Err(error) = runtime.dispatch(AppCommand::DisablePlugin { plugin_id }) {
                        self.serial.borrow_mut().status = error;
                    }
                }
                PluginPanelEvent::RefreshMarket => {
                    self.request_web_marketplace_refresh(ui.ctx());
                }
                PluginPanelEvent::ImportPlugin => {
                    self.request_web_plugin_files(ui.ctx());
                }
                PluginPanelEvent::MarketplaceUrlChanged(url) => {
                    self.marketplace_url = url;
                    self.persist_settings();
                }
                PluginPanelEvent::InstallPlugin(plugin_id) => {
                    if let Some(entry) = marketplace_view.registry.as_ref().and_then(|registry| {
                        registry.plugins.iter().find(|entry| entry.id == plugin_id)
                    }) {
                        let Some(manifest_url) = entry.manifest_url.clone() else {
                            self.serial.borrow_mut().status =
                                format!("插件 {} 缺少统一 plugin.json，无法安装", entry.id);
                            continue;
                        };
                        let Some(main_url) = entry.main_url.clone() else {
                            self.serial.borrow_mut().status =
                                format!("插件 {} 缺少统一 main.lua，无法安装", entry.id);
                            continue;
                        };
                        self.install_web_marketplace_entry(
                            entry.id.clone(),
                            manifest_url,
                            main_url,
                        );
                    }
                }
                PluginPanelEvent::UninstallPlugin(plugin_id) => {
                    self.uninstall_web_plugin(&plugin_id);
                }
            }
        }
        // The shared plugin panel represents Restart as Disable followed by
        // Enable once the old runtime has actually gone away. Native performs
        // this hand-off in its composition root; Web must do the same instead
        // of silently dropping the pending restart request.
        for plugin_id in self.plugins_panel.take_pending_restart() {
            if let Err(error) = runtime.dispatch(AppCommand::EnablePlugin { plugin_id }) {
                self.serial.borrow_mut().status = error;
            }
        }
    }

    pub(crate) fn web_plugin_settings_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = Vec::new();
        let mut has_settings = false;
        let ports = self
            .runtime
            .as_ref()
            .map(WebRuntime::query_transport)
            .map(|view| {
                view.ports
                    .iter()
                    .map(|port| port.id.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_else(|| {
                self.serial
                    .borrow()
                    .ports
                    .iter()
                    .map(|port| port.id.to_string())
                    .collect()
            });
        for (index, record) in self.plugins.records.iter_mut().enumerate() {
            let settings: Vec<PluginSettingView> = record
                .persisted
                .manifest
                .contributes
                .settings
                .iter()
                .map(plugin_setting_view)
                .collect();
            if settings.is_empty() {
                continue;
            }
            has_settings = true;
            let before = record.persisted.settings.clone();
            let plugin_id = record.persisted.manifest.id.clone();
            let plugin_name = record.persisted.manifest.name.clone();
            let mut view = PluginSettingsView {
                plugin_id: &plugin_id,
                plugin_name: &plugin_name,
                settings: &settings,
                ports: &ports,
                values: &mut record.persisted.settings,
            };
            plugin_settings_ui(ui, &mut view);
            if record.persisted.settings != before {
                changed.push((
                    index,
                    serde_json::to_string(&record.persisted.settings)
                        .unwrap_or_else(|_| "{}".to_owned()),
                ));
            }
        }
        if !has_settings {
            design::card().show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                design::empty_state(ui, ICON_APPS, "暂无插件设置");
            });
        }
        for (index, settings_json) in changed {
            if let Some(instance) = self
                .plugins
                .records
                .get(index)
                .and_then(|record| record.lua_instance)
            {
                let settings = serde_json::from_str::<serde_json::Value>(&settings_json)
                    .map(|value| PluginValue::from_json(&value));
                if let Ok(settings) = settings
                    && let Err(error) = self.web_lua.update_settings(instance, settings)
                {
                    self.serial.borrow_mut().status = error.to_string();
                }
            }
            self.persist_settings();
        }
    }

    fn web_ui_contribution_context(&self, slot: &str) -> serde_json::Value {
        let serial = self.serial.borrow();
        let transport = self.runtime.as_ref().map(WebRuntime::query_transport);
        let connected = web_connected_port(transport.as_ref(), &serial);
        // The sender target is user-selectable.  Falling back to the active
        // connection keeps the plugin usable immediately after connect, but
        // an explicitly selected unopened port must remain unopened in the
        // command context.
        let target_port = serial.selected_port.clone().or_else(|| connected.clone());
        let target_port_open = target_port
            .as_ref()
            .is_some_and(|port| connected.as_ref() == Some(port));
        let open_ports = transport
            .as_ref()
            .map(|view| {
                view.ports
                    .iter()
                    .filter(|port| port.kind == PortKind::Serial)
                    .map(|port| port.id.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if slot.starts_with("send.") {
            serde_json::json!({
                "slot": slot,
                "send": {
                    "input": serial.send_input,
                    "target_port": target_port,
                    "target_port_open": target_port_open,
                    "hex_mode": serial.tx_hex,
                    "line_ending": {
                        "label": serial.line_ending.label(),
                        "suffix": serial.line_ending.suffix(),
                    },
                    "periodic_enabled": serial.periodic_enabled,
                    "periodic_interval_ms": serial.periodic_interval_ms,
                },
                "serial": {
                    "selected_port": serial.selected_port,
                    "open_ports": open_ports,
                }
            })
        } else {
            serde_json::json!({ "slot": slot })
        }
    }

    /// 贡献点命令的 payload `context`：在槽位快照上补上来源与被点击的贡献点 id，
    /// 与 Native 派发给 Lua 的形状一致。
    pub(crate) fn web_contribution_command_context(
        &self,
        slot: &str,
        contribution_id: &str,
    ) -> serde_json::Value {
        let mut context = self.web_ui_contribution_context(slot);
        if let Some(object) = context.as_object_mut() {
            object.insert(
                "source".to_owned(),
                serde_json::json!("web_ui_contribution"),
            );
            object.insert(
                "contribution_id".to_owned(),
                serde_json::json!(contribution_id),
            );
        }
        context
    }

    /// 槽位上可见的贡献点：只取运行中插件，按 order 再按展示名排序——
    /// 排序与渲染必须同源，`send.toolbar` 的按钮由 `shared_sender_ui` 自己画。
    fn web_ui_contributions_for_slot(&self, slot: &str) -> Vec<(String, PluginUiContributionView)> {
        let mut items = self
            .plugins
            .summaries()
            .into_iter()
            .filter(|summary| summary.state == PluginStateView::Running)
            .flat_map(|summary| {
                let plugin_id = summary.id;
                summary
                    .contributes
                    .ui
                    .into_iter()
                    .filter(|item| {
                        item.slot == slot
                            && item.visible
                            && !(slot == "send.toolbar"
                                && matches!(
                                    item.kind.to_ascii_lowercase().as_str(),
                                    "button" | "small_button" | ""
                                ))
                    })
                    .map(move |item| (plugin_id.clone(), item))
            })
            .collect::<Vec<_>>();
        items.sort_by(|(_, left), (_, right)| {
            left.order
                .cmp(&right.order)
                .then_with(|| contribution_title(left).cmp(contribution_title(right)))
        });
        items
    }

    /// Render the same chrome contribution slots that Native exposes. Lua
    /// contribution state stays in the browser composition root; the visible
    /// contract (slot, ordering, labels and command dispatch) remains identical.
    pub(crate) fn web_ui_contribution_slot(&mut self, ui: &mut egui::Ui, slot: &str) {
        let mut commands = Vec::new();
        for (plugin_id, item) in self.web_ui_contributions_for_slot(slot) {
            let PluginUiContributionView {
                id: contribution_id,
                kind,
                title,
                command,
                tooltip,
                enabled,
                default,
                ..
            } = item;
            let title = title.unwrap_or_else(|| contribution_id.clone());
            let state = self
                .plugins
                .contribution_value(&plugin_id, &contribution_id)
                .cloned()
                .unwrap_or(default);
            match kind.to_ascii_lowercase().as_str() {
                "separator" => {
                    ui.separator();
                }
                "label" | "status" => {
                    let display = state.as_str().unwrap_or(&title);
                    ui.label(egui::RichText::new(display).color(theme::text_secondary()));
                }
                "progress" => {
                    let value = state
                        .get("value")
                        .and_then(serde_json::Value::as_f64)
                        .or_else(|| state.as_f64())
                        .unwrap_or(0.0)
                        .clamp(0.0, 1.0) as f32;
                    let text = state
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("");
                    if state.get("visible").and_then(serde_json::Value::as_bool) == Some(false)
                        || (value <= 0.0 && text.is_empty())
                    {
                        continue;
                    }
                    ui.add(
                        egui::ProgressBar::new(value)
                            .desired_width(52.0)
                            .desired_height(8.0)
                            .text(text.to_owned()),
                    );
                }
                "toggle" => {
                    let current = state.as_bool().unwrap_or(false);
                    if ui.selectable_label(current, title).clicked() {
                        self.plugins.set_contribution_value(
                            &plugin_id,
                            &contribution_id,
                            serde_json::json!(!current),
                        );
                        if let Some(command) = command {
                            commands.push((plugin_id, command, contribution_id));
                        }
                    }
                }
                "button" | "small_button" | "" => {
                    let response = ui.add_enabled(enabled, egui::Button::new(title));
                    let response = if let Some(tooltip) = tooltip {
                        response.on_hover_text(tooltip)
                    } else {
                        response
                    };
                    if response.clicked()
                        && let Some(command) = command
                    {
                        commands.push((plugin_id, command, contribution_id));
                    }
                }
                _ => {
                    ui.add_enabled(false, egui::Button::new(title));
                }
            }
        }
        for (plugin_id, command_id, contribution_id) in commands {
            let context = self.web_contribution_command_context(slot, &contribution_id);
            // Browser file/serial capability requests must be started in the
            // same UI turn as the clicked contribution. Queueing the command
            // in WebRuntime would move requestPort()/input.click() to the
            // next frame, after transient user activation expires.
            self.execute_web_plugin_command(
                plugin_id.clone(),
                command_id.clone(),
                serde_json::json!({
                    "plugin_id": plugin_id.clone(),
                    "contribution_id": contribution_id,
                    "slot": slot,
                    "kind": "button",
                    "command": command_id.clone(),
                    "context": context,
                }),
            );
            self.poll_web_lua_host_requests();
        }
    }

    /// Execute a Web Lua command at the composition root.
    ///
    /// Ordinary commands may be queued and handled by `poll_web_plugins`, but
    /// commands originating from a visible browser button need an immediate
    /// path: a plugin can synchronously create a pending file picker or
    /// `requestPort()` request, and both APIs require the current user gesture.
    pub(crate) fn execute_web_plugin_command(
        &mut self,
        plugin_id: String,
        command_id: String,
        context: serde_json::Value,
    ) {
        let Some(index) = self.web_plugin_index(&plugin_id) else {
            self.serial.borrow_mut().status = format!("Web 插件不存在：{plugin_id}");
            return;
        };
        let Some(instance) = self
            .plugins
            .records
            .get(index)
            .and_then(|record| record.lua_instance)
        else {
            self.serial.borrow_mut().status = format!("Web 插件未运行：{plugin_id}");
            return;
        };

        let context = PluginValue::from_json(&context);
        match self
            .web_lua
            .dispatch_command(instance, &command_id, context)
        {
            Ok(PluginCallResult::Completed(value)) => {
                self.serial.borrow_mut().status = format!(
                    "{}：{}",
                    plugin_id,
                    value
                        .to_json()
                        .map(|value| value.to_string())
                        .unwrap_or_default()
                );
            }
            Ok(PluginCallResult::Yielded { .. }) => {
                self.serial.borrow_mut().status = format!("{plugin_id}：命令已进入异步任务");
            }
            Err(error) => self.serial.borrow_mut().status = error.to_string(),
        }
    }
}

/// 以失败结束一条挂起的 Lua 宿主请求。宿主自身的错误直接丢弃：这条路径上的失败原因
/// 已经通过 `completion` 本身回报给 Lua，再报告一次只会覆盖真正的错误。
fn fail_lua_host_request(host: &WebPluginHost, request_id: &str, error: &str) {
    let _ = host.complete_request(PluginHostCompletion {
        request_id: request_id.to_owned(),
        result: Err(error.to_owned()),
    });
}

/// 浏览器能兑现的能力只有下面这十项：插件声明超出这份白名单时，
/// 直接给出拒绝加载的错误文案，而不是悄悄少给一项权限。
fn web_missing_capability_error(permissions: &PluginPermissions) -> Option<String> {
    let missing = permissions.missing_from([
        tool_plugin_api::PluginCapability::Bus,
        tool_plugin_api::PluginCapability::Config,
        tool_plugin_api::PluginCapability::Dialog,
        tool_plugin_api::PluginCapability::Filesystem,
        tool_plugin_api::PluginCapability::Log,
        tool_plugin_api::PluginCapability::Serial,
        tool_plugin_api::PluginCapability::Storage,
        tool_plugin_api::PluginCapability::Task,
        tool_plugin_api::PluginCapability::Timer,
        tool_plugin_api::PluginCapability::Ui,
    ]);
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "插件声明了当前浏览器不支持的权限：{}",
        missing
            .iter()
            .map(|capability| capability.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// 浏览器清单里的设置项 → 共享 DTO。缺省值缺失时按 JSON null 处理，
/// 与 Native 插件设置面板读到的是同一个形状。
pub(crate) fn plugin_setting_view(setting: &WebPluginSetting) -> PluginSettingView {
    PluginSettingView {
        id: setting.id.clone(),
        title: setting.title.clone(),
        kind: setting.kind.clone(),
        default: setting.default.clone().unwrap_or(serde_json::Value::Null),
        options: setting.options.clone(),
        min: setting.min,
        max: setting.max,
        step: setting.step,
        rows: setting.rows,
        description: setting.description.clone(),
    }
}

/// 贡献点的展示名：标题缺省时回落到 id。排序与渲染必须用同一个名字。
fn contribution_title(item: &PluginUiContributionView) -> &str {
    item.title.as_deref().unwrap_or(&item.id)
}

/// 取浏览器 `File.name` 的叶子名（去掉目录前缀）。清单与入口脚本都只按叶子名匹配。
fn web_file_leaf(name: &str) -> &str {
    name.rsplit_once('/').map(|(_, leaf)| leaf).unwrap_or(name)
}

fn parse_web_plugin_files(files: &[(String, String)]) -> Result<WebPluginPersisted, String> {
    let manifest_text = files
        .iter()
        .find(|(name, _)| web_file_leaf(name).eq_ignore_ascii_case("plugin.json"))
        .or_else(|| {
            files
                .iter()
                .find(|(name, _)| name.ends_with(".json") || name.ends_with(".JSON"))
        })
        .map(|(_, text)| text)
        .ok_or_else(|| "Web 插件缺少 plugin.json".to_owned())?;
    let manifest = serde_json::from_str::<WebPluginManifest>(manifest_text)
        .map_err(|error| format!("解析 Web 插件清单失败：{error}"))?;
    if manifest.id.trim().is_empty()
        || manifest.name.trim().is_empty()
        || manifest.version.trim().is_empty()
    {
        return Err("Web 插件清单必须包含 id、name 和 version".to_owned());
    }
    if manifest.runtime != "lua" {
        return Err(format!(
            "插件 {} 使用 runtime={}，浏览器只接受 runtime=lua；Web 与 Native 使用同一 main.lua",
            manifest.id, manifest.runtime
        ));
    }
    if manifest.api_version != WEB_PLUGIN_API_VERSION && manifest.api_version != "0.1" {
        return Err(format!(
            "插件 {} 使用插件 API {}，当前浏览器支持 {} / 0.1",
            manifest.id, manifest.api_version, WEB_PLUGIN_API_VERSION
        ));
    }
    let main_name = web_file_leaf(manifest.live_main());
    let source = files
        .iter()
        .find(|(name, _)| web_file_leaf(name) == main_name)
        .or_else(|| {
            files
                .iter()
                .find(|(name, _)| name.ends_with(".lua") || name.ends_with(".LUA"))
        })
        .map(|(_, text)| text.clone())
        .ok_or_else(|| format!("Web 插件缺少入口文件：{}", manifest.live_main()))?;
    let replay_source = manifest.replay.as_ref().and_then(|replay| {
        let replay_name = web_file_leaf(&replay.main);
        files
            .iter()
            .find_map(|(name, text)| (web_file_leaf(name) == replay_name).then_some(text.clone()))
    });
    if manifest.replay.is_some() && replay_source.is_none() {
        return Err(format!(
            "Web 插件声明了 replay，但缺少回放入口文件：{}",
            manifest
                .replay
                .as_ref()
                .map(|replay| replay.main.as_str())
                .unwrap_or("replay.lua")
        ));
    }
    let settings = manifest
        .contributes
        .settings
        .iter()
        .filter_map(|setting| {
            setting
                .default
                .clone()
                .map(|value| (setting.id.clone(), value))
        })
        .collect();
    Ok(WebPluginPersisted {
        manifest,
        source,
        replay_source,
        enabled: true,
        settings,
        storage: BTreeMap::new(),
        profiles: BTreeMap::new(),
    })
}

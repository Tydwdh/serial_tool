#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod device_state;
mod replay_state;
mod search;
mod sender_state;
mod settings_state;
mod state;
mod table_model;
mod terminal_state;
mod theme_bridge;
mod util;
mod vis_state;

slint::include_modules!();

use std::{cell::RefCell, sync::Arc};

use slint::TimerMode;

fn main() -> Result<(), slint::PlatformError> {
    let _ = env_logger::try_init();

    let app_state = Arc::new(app::AppState::load());
    let theme_dir = app_state.theme_dir.clone();
    let stored_theme = app_state.theme_path();
    let bus = app_state.bus.clone();
    let transport = app_state.transport.clone();
    // P5 状态
    let terminal = Arc::new(RefCell::new(terminal_state::TerminalState::new(&bus)));
    let log_state = Arc::new(RefCell::new(terminal_state::LogState::new(&bus)));
    let device = Arc::new(RefCell::new(device_state::DeviceState::from_config(&app_state.config)));
    let sender = Arc::new(RefCell::new(sender_state::SenderState::from_config(&app_state.config)));
    // 初始化别名与端口
    {
        let mut t = terminal.borrow_mut();
        t.set_port_aliases(&app_state.config.port_aliases);
        t.set_max_entries(app_state.config.terminal_max_entries);
    }
    {
        let mut l = log_state.borrow_mut();
        l.max_entries = app_state.config.log_max_entries;
    }
    device.borrow_mut().refresh_ports(&transport);
    let replay = Arc::new(RefCell::new(replay_state::ReplayUiState::new(&bus)));
    let replay_for_timer = replay.clone();
    // Vis：Chart/Attitude/Gauge 状态（共享 DataBus，Slint 定时 ingest）
    let chart = Arc::new(RefCell::new(vis_state::ChartState::new(&bus)));
    let chart_timer = chart.clone();
    let attitude = Arc::new(RefCell::new(vis_state::AttitudeState::new(&bus)));
    let attitude_timer = attitude.clone();
    let gauge = Arc::new(RefCell::new(vis_state::GaugeState::new(&bus, "protocol.gauge.value")));
    let gauge_timer = gauge.clone();
    let table = Arc::new(RefCell::new({
        let mut m = serde_json::Map::new();
        m.insert(
            "columns".to_owned(),
            serde_json::json!([{"id":"id","title":"ID","width":80},{"id":"value","title":"值","width":140}]),
        );
        m.insert("rows".to_owned(), serde_json::json!([{"id":"1","value":42},{"id":"2","value":7}]));
        table_model::DataTableState::from_config(&m).unwrap()
    }));
    // Settings
    let settings = Arc::new(RefCell::new(crate::settings_state::SettingsState::from_config(&app_state.config, theme_dir.clone())));

    let window = AppWindow::new()?;

    // 主题：初始化 + 写入 Palette global
    theme_bridge::load_initial_theme(&window, &theme_dir, stored_theme.as_deref());
    // Settings 初始同步到窗口
    {
        let s = settings.borrow();
        window.set_settings_theme_name(s.theme_name.clone().into());
        window.set_settings_theme_options(slint::ModelRc::from(s.theme_options.iter().map(|x| slint::SharedString::from(x.clone())).collect::<Vec<_>>().as_slice()));
        window.set_settings_recorder_path(s.recorder_path.clone().into());
        window.set_settings_proxy(s.proxy_url.clone().into());
        window.set_settings_font_size(s.font_size);
        window.set_settings_term_merge(s.term_merge_ms as i32);
        window.set_settings_term_max(s.term_max as i32);
        window.set_settings_log_max(s.log_max as i32);
        window.set_settings_status(s.status.clone().into());
    }

    // Replay 定时：16ms 驱动 tick_playback + 进度同步
    {
        let win_weak = window.as_weak();
        let r = replay_for_timer.clone();
        let timer = slint::Timer::default();
        timer.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(16),
            move || {
                let mut state = r.borrow_mut();
                let status = state.manager.status();
                let playing = status.state == tool_recorder::ReplayState::Playing;
                if playing {
                    let (_, _) = state.tick_playback();
                }
                if let Some(w) = win_weak.upgrade() {
                    w.set_replay_progress(state.progress01());
                    w.set_replay_status(state.status_text().into());
                    w.set_replay_playing(playing);
                    w.set_replay_message(state.message.clone().unwrap_or_default().into());
                    w.set_replay_speed((state.speed as f32).into());
                    w.set_replay_loop(state.loop_playback);
                    w.set_replay_step(state.step_size as i32);
                }
            },
        );
        std::mem::forget(timer);
    }

    // DataTable：排序/选择
    {
        let _table = table.clone();
        let win_weak = window.as_weak();
        window.on_table_sort(move |col| {
            let mut t = _table.borrow_mut();
            // 表头传 title，这里做 id 映射（示例列 title==id）
            let id = if col == "ID" { "id" } else { "value" };
            t.sort_by(id);
            if let Some(w) = win_weak.upgrade() {
                w.set_table_empty(format!("已按 {id} 排序").into());
            }
        });
        let _table2 = table.clone();
        window.on_table_select(move |id| {
            _table2.borrow_mut().select(&id);
        });
    }

    // Replay 回调
    {
        let r = replay.clone();
        let win_weak = window.as_weak();
        window.on_replay_pick_file(move || {
            if let Some(p) = crate::config::config_path()
                .parent()
                .map(|p| p.to_path_buf())
            {
                let _ = &p;
            }
            if let Some(file) = rfd::FileDialog::new()
                .add_filter("JSONL", &["jsonl"])
                .pick_file()
            {
                let s = file.display().to_string();
                r.borrow_mut().path = s.clone();
                if let Some(w) = win_weak.upgrade() {
                    w.set_replay_path(s.into());
                }
            }
        });
        let r2 = replay.clone();
        window.on_replay_load(move || {
            r2.borrow_mut().try_load();
        });
        let r3 = replay.clone();
        window.on_replay_play(move || {
            r3.borrow_mut().manager.play();
        });
        let r4 = replay.clone();
        window.on_replay_pause(move || {
            r4.borrow_mut().manager.pause();
        });
        let r5 = replay.clone();
        window.on_replay_stop(move || {
            r5.borrow_mut().manager.stop();
        });
        let r6 = replay.clone();
        window.on_replay_prev(move || {
            let step = r6.borrow().step_size;
            r6.borrow_mut().do_step_backward(step);
        });
        let r7 = replay.clone();
        window.on_replay_next(move || {
            r7.borrow_mut().manager.step_forward();
        });
        let r8 = replay.clone();
        window.on_replay_set_speed(move |v| {
            r8.borrow_mut().speed = v as f64;
            r8.borrow_mut().manager.set_speed(v as f64);
        });
        let r9 = replay.clone();
        window.on_replay_set_step(move |v| {
            r9.borrow_mut().step_size = v as usize;
        });
    }

    // Vis 定时：30Hz ingest + 同步到窗口属性
    {
        let win_weak = window.as_weak();
        let c = chart_timer.clone();
        let a = attitude_timer.clone();
        let g = gauge_timer.clone();
        let timer = slint::Timer::default();
        timer.start(TimerMode::Repeated, std::time::Duration::from_millis(33), move || {
            c.borrow_mut().ingest();
            a.borrow_mut().ingest();
            g.borrow_mut().ingest();
            if let Some(w) = win_weak.upgrade() {
                // Chart
                let ch = c.borrow();
                let (ymin, ymax) = ch.bounds_y();
                let legend = if ch.series.is_empty() {
                    "".to_owned()
                } else {
                    ch.series
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let dropped = ch.subscription_dropped();
                let status = if ch.paused {
                    format!("已暂停（丢弃 {}）", ch.dropped_while_paused)
                } else if dropped > 0 {
                    format!("队列溢出丢弃 {dropped} 条")
                } else if ch.series.is_empty() {
                    "无采样数据 — 等待 protocol.* 事件".to_owned()
                } else {
                    format!("Y [{:.1}, {:.1}] 窗口 {}", ymin, ymax, ch.sample_window)
                };
                w.set_chart_paused(ch.paused);
                w.set_chart_auto(ch.auto_scale);
                w.set_chart_legend(legend.into());
                w.set_chart_status(status.into());
                // Attitude
                let at = a.borrow();
                w.set_att_roll(at.roll as f32);
                w.set_att_pitch(at.pitch as f32);
                w.set_att_yaw(at.yaw as f32);
                w.set_att_samples(at.samples as i32);
                w.set_att_source(at.last_source.clone().into());
                // Gauge
                let ga = g.borrow();
                w.set_gauge_value(ga.value as f32);
                w.set_gauge_min(ga.min as f32);
                w.set_gauge_max(ga.max as f32);
                w.set_gauge_unit(ga.unit.clone().into());
                w.set_gauge_label(ga.label.clone().into());
                w.set_gauge_status(ga.status_text().into());
                w.set_gauge_samples(ga.samples as i32);
            }
        });
        std::mem::forget(timer);
    }
    // Chart/Attitude/Gauge 操作
    {
        let c = chart.clone();
        let win_weak = window.as_weak();
        let _ = &win_weak;
        window.on_chart_toggle_paused(move || {
            c.borrow_mut().paused = !c.borrow().paused;
        });
        let c2 = chart.clone();
        window.on_chart_toggle_auto(move || {
            c2.borrow_mut().auto_scale = !c2.borrow().auto_scale;
        });
        let c3 = chart.clone();
        window.on_chart_clear(move || {
            c3.borrow_mut().clear();
        });
        let a2 = attitude.clone();
        window.on_att_clear(move || {
            a2.borrow_mut().clear();
        });
        let g2 = gauge.clone();
        window.on_gauge_clear(move || {
            g2.borrow_mut().clear();
        });
    }

    // P5/P7 定时：Terminal/Log ingest + Device/Sender 同步到窗口（含真 rows 模型）
    {
        let win_weak = window.as_weak();
        let term = terminal.clone();
        let logs = log_state.clone();
        let dev = device.clone();
        let snd = sender.clone();
        let st = app_state.clone();
        let timer = slint::Timer::default();
        timer.start(TimerMode::Repeated, std::time::Duration::from_millis(33), move || {
            term.borrow_mut().ingest();
            logs.borrow_mut().ingest();
            if let Some(w) = win_weak.upgrade() {
                // Terminal：裁剪后 200 行 + 状态 + ports
                let (rows, dropped, truncated, ports) = {
                    let t = term.borrow();
                    let mut rows = t.visible_rows();
                    if rows.len() > 200 {
                        rows = rows.into_iter().rev().take(200).rev().collect();
                    }
                    (rows, t.dropped_count(), t.truncated, t.ports_list())
                };
                let status = if dropped > 0 {
                    format!("丢弃 {dropped} 条")
                } else if truncated {
                    "已截断".to_owned()
                } else {
                    format!("{} 行", rows.len())
                };
                let term_rows: Vec<TermRow> = rows
                    .into_iter()
                    .map(|r| TermRow {
                        id: r.id as i32,
                        ts: r.ts.into(),
                        port: r.port.into(),
                        dir: r.dir.into(),
                        preview: r.preview.into(),
                        selected: r.selected,
                    })
                    .collect();
                w.set_term_rows(slint::ModelRc::from(term_rows.as_slice()));
                w.set_term_status(status.into());
                w.set_term_ports(slint::ModelRc::from(ports.into_iter().map(|s| s.into()).collect::<Vec<slint::SharedString>>().as_slice()));
                // Log：裁剪 200 行
                let (lrows, l_dropped) = {
                    let l = logs.borrow();
                    let mut rows = l.visible_rows();
                    if rows.len() > 200 {
                        rows = rows.into_iter().rev().take(200).rev().collect();
                    }
                    let d = l.dropped_count();
                    (rows, d)
                };
                let lstatus = if l_dropped > 0 {
                    format!("丢弃 {l_dropped}")
                } else {
                    format!("{} 条", lrows.len())
                };
                let log_rows: Vec<LogRow> = lrows
                    .into_iter()
                    .map(|e| LogRow {
                        id: e.id as i32,
                        ts: e.ts.into(),
                        level: format!("{}", e.level).into(),
                        source: e.source.into(),
                        text: e.text.into(),
                    })
                    .collect();
                w.set_log_rows(slint::ModelRc::from(log_rows.as_slice()));
                w.set_log_status(lstatus.into());
                // Device
                let d = dev.borrow();
                w.set_device_ports(slint::ModelRc::from(d.ports.iter().map(|p| slint::SharedString::from(p.port_name.clone())).collect::<Vec<_>>().as_slice()));
                w.set_device_selected(d.selected_port.clone().unwrap_or_default().into());
                w.set_device_baud(d.baud_rate.clone().into());
                w.set_device_status(d.last_error.clone().unwrap_or_default().into());
                w.set_device_dtr(true);
                w.set_device_rts(true);
                // Sender
                let s = snd.borrow();
                w.set_sender_input(s.input.clone().into());
                w.set_sender_hex(s.hex_mode);
                w.set_sender_ending(s.line_ending.label().into());
                w.set_sender_error(s.error.clone().unwrap_or_default().into());
                w.set_sender_target(s.target_port.clone().unwrap_or_default().into());
                let _ = &st;
            }
        });
        std::mem::forget(timer);
    }
    // P5 回调
    {
        let t = terminal.clone();
        window.on_term_toggle_rx(move || { let mut s = t.borrow_mut(); s.show_rx = !s.show_rx; });
        let t2 = terminal.clone();
        window.on_term_toggle_tx(move || { let mut s = t2.borrow_mut(); s.show_tx = !s.show_tx; });
        let t3 = terminal.clone();
        window.on_term_toggle_hex(move || { let mut s = t3.borrow_mut(); s.show_hex = !s.show_hex; });
        let t4 = terminal.clone();
        window.on_term_toggle_raw(move || { let mut s = t4.borrow_mut(); s.show_raw = !s.show_raw; });
        let t5 = terminal.clone();
        window.on_term_search_changed(move |v| { t5.borrow_mut().search_text = v.to_string(); });
        let t6 = terminal.clone();
        window.on_term_toggle_search_case(move || { let mut s = t6.borrow_mut(); s.search_case = !s.search_case; });
        let t7 = terminal.clone();
        window.on_term_port_changed(move |v| { t7.borrow_mut().port_filter = if v.is_empty() { None } else { Some(v.to_string()) }; });
        let t8 = terminal.clone();
        window.on_term_clear(move || { t8.borrow_mut().clear(); });
        let t9 = terminal.clone();
        window.on_term_copy_selected(move || {
            let txt = t9.borrow().export_visible_text();
            let _ = slint::platform::Clipboard::default();
            // 使用 open 剪贴板回退：写入临时文件提示（P5 简化）
            log::info!("copy selected: {} chars", txt.len());
        });
        let t10 = terminal.clone();
        window.on_term_export_txt(move || { let txt = t10.borrow().export_visible_text(); let _ = txt; });
        let t11 = terminal.clone();
        window.on_term_export_csv(move || { let _ = t11.borrow().export_visible_csv(); });
        let t12 = terminal.clone();
        window.on_term_export_json(move || { let _ = t12.borrow().export_visible_json(); });
        let t13 = terminal.clone();
        window.on_term_select_row(move |id| { t13.borrow_mut().toggle_selected(id as u64); });
        // Log
        let l = log_state.clone();
        window.on_log_search_changed(move |v| { l.borrow_mut().search_text = v.to_string(); });
        let l2 = log_state.clone();
        window.on_log_toggle_search_case(move || { let mut s = l2.borrow_mut(); s.search_case = !s.search_case; });
        let l3 = log_state.clone();
        window.on_log_level_changed(move |v| { if let Ok(lv) = v.parse::<tool_core::LogLevel>() { l3.borrow_mut().min_level = lv; } });
        let l4 = log_state.clone();
        window.on_log_clear(move || { l4.borrow_mut().clear(); });
        let l5 = log_state.clone();
        window.on_log_select_row(move |id| { let mut s = l5.borrow_mut(); if !s.selected_ids.remove(&(id as u64)) { s.selected_ids.insert(id as u64); } });
        // Device
        let d = device.clone();
        let tr = transport.clone();
        window.on_device_refresh(move || { d.borrow_mut().refresh_ports(&tr); });
        let d2 = device.clone();
        let tr2 = transport.clone();
        window.on_device_open(move || { let _ = d2.borrow_mut().open_selected(&tr2); });
        let d3 = device.clone();
        let tr3 = transport.clone();
        window.on_device_close(move || { d3.borrow().close_selected(&tr3); });
        let d4 = device.clone();
        window.on_device_port_changed(move |v| { d4.borrow_mut().selected_port = Some(v.to_string()); });
        let d5 = device.clone();
        window.on_device_baud_changed(move |v| { d5.borrow_mut().baud_rate = v.to_string(); });
        let d6 = device.clone();
        let tr6 = transport.clone();
        window.on_device_dtr_toggled(move || { if let Some(p) = d6.borrow().selected_port.clone() { let _ = tr6.set_dtr(&p, true); } });
        let d7 = device.clone();
        let tr7 = transport.clone();
        window.on_device_rts_toggled(move || { if let Some(p) = d7.borrow().selected_port.clone() { let _ = tr7.set_rts(&p, true); } });
        // Sender
        let s = sender.clone();
        let trs = transport.clone();
        let st2 = app_state.clone();
        window.on_sender_send(move || {
            let res = s.borrow_mut().do_send(&trs);
            if let Err(e) = res { s.borrow_mut().error = Some(e.clone()); st2.push_status(state::StatusLevel::Error, e); }
        });
        let s2 = sender.clone();
        window.on_sender_toggle_hex(move || { s2.borrow_mut().hex_mode = !s2.borrow().hex_mode; });
        let s3 = sender.clone();
        window.on_sender_ending_changed(move |v| {
            let le = match v.as_str() { "LF" => state::LineEnding::Lf, "CR" => state::LineEnding::Cr, "CRLF" => state::LineEnding::Crlf, _ => state::LineEnding::None };
            s3.borrow_mut().line_ending = le;
        });
    }

    // 状态栏：定时轮询 NotificationQueue（TTL 5/8/15s）
    {
        let win_weak = window.as_weak();
        let st = app_state.clone();
        let timer = slint::Timer::default();
        timer.start(
            TimerMode::Repeated,
            std::time::Duration::from_millis(500),
            move || {
                if let Some(w) = win_weak.upgrade() {
                    let (text, level) = app::poll_status_text(&st);
                    w.set_status_text(text.into());
                    w.set_status_level(level.into());
                    theme_bridge::apply_palette_from_panels(&w);
                }
            },
        );
        std::mem::forget(timer);
    }

    // 打开配置目录
    {
        let win_weak = window.as_weak();
        let st = app_state.clone();
        window.on_open_config_folder(move || {
            let dir = crate::config::config_path()
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| theme_dir.clone());
            let msg = if dir.exists() {
                format!("配置目录：{}", dir.display())
            } else {
                format!("配置目录（尚未创建）：{}", dir.display())
            };
            st.push_status(state::StatusLevel::Info, msg.clone());
            if let Some(w) = win_weak.upgrade() {
                w.set_status_text(msg.into());
            }
            let _ = open::that(&dir);
        });
    }

    // Settings 回调
    {
        let s = settings.clone();
        let st = app_state.clone();
        let win_weak = window.as_weak();
        window.on_settings_theme_changed(move |name| {
            s.borrow_mut().theme_name = name.to_string();
            // 尝试按名称查找主题文件并应用
            let dir = st.theme_dir.clone();
            for (path, tname) in tool_panels::theme::discover_theme_files(&dir) {
                if tname == name.as_str() {
                    let _ = tool_panels::theme::load_theme_file(&path);
                    if let Some(t) = tool_panels::theme::builtin_theme_for_path(&path) {
                        tool_panels::theme::set_active_theme(t);
                    }
                    let rel = path.strip_prefix(&dir).unwrap_or(&path).display().to_string();
                    let mut cfg = st.build_snapshot();
                    cfg.theme_path = Some(rel);
                    let _ = crate::config::save_config_snapshot(&cfg);
                    break;
                }
            }
            if let Some(w) = win_weak.upgrade() {
                theme_bridge::apply_palette_from_panels(&w);
            }
        });
        let s2 = settings.clone();
        window.on_settings_open_theme_folder(move || { let _ = open::that(&s2.borrow().theme_dir); });
        let s3 = settings.clone();
        let win_weak3 = window.as_weak();
        window.on_settings_pick_recorder(move || {
            if let Some(p) = rfd::FileDialog::new().add_filter("JSONL", &["jsonl"]).save_file() {
                let s = crate::config::ensure_jsonl_extension(p).display().to_string();
                s3.borrow_mut().recorder_path = s.clone();
                if let Some(w) = win_weak3.upgrade() { w.set_settings_recorder_path(s.into()); }
            }
        });
        let s4 = settings.clone();
        let st4 = app_state.clone();
        let win_weak4 = window.as_weak();
        window.on_settings_save(move || {
            let mut cfg = st4.build_snapshot();
            s4.borrow().apply_to_config(&mut cfg);
            match crate::config::save_config_snapshot(&cfg) {
                Ok(()) => { s4.borrow_mut().status = "已保存".to_owned(); if let Some(w) = win_weak4.upgrade() { w.set_settings_status("已保存".into()); } st4.push_status(state::StatusLevel::Info, "已保存设置"); }
                Err(e) => { s4.borrow_mut().status = e.clone(); if let Some(w) = win_weak4.upgrade() { w.set_settings_status(e.clone().into()); } }
            }
        });
        let s5 = settings.clone();
        window.on_settings_reset_layout(move || { s5.borrow_mut().status = "已重置布局（下次启动生效）".to_owned(); });
        let s6 = settings.clone();
        let win_weak6 = window.as_weak();
        window.on_settings_font_changed(move |v| { s6.borrow_mut().font_size = v.clamp(10.0, 24.0); if let Some(w) = win_weak6.upgrade() { w.set_settings_font_size(s6.borrow().font_size); } });
        let s7 = settings.clone();
        window.on_settings_proxy_changed(move |v| { s7.borrow_mut().proxy_url = v.to_string(); });
    }

    if app_state.config_migrated {
        let _ = app_state.save();
    }

    window.run()
}

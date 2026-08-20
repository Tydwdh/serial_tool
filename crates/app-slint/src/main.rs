#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod replay_state;
mod search;
mod state;
mod table_model;
mod theme_bridge;
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
    let search_state = Arc::new(RefCell::new((String::new(), false))); // (query, case_sensitive)

    let window = AppWindow::new()?;

    // 主题：初始化 + 写入 Palette global
    theme_bridge::load_initial_theme(&window, &theme_dir, stored_theme.as_deref());

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

    // 搜索：re: 正则 + 大小写
    {
        let win_weak = window.as_weak();
        let ss = search_state.clone();
        let st = app_state.clone();
        window.on_search_changed(move |text| {
            ss.borrow_mut().0 = text.to_string();
            let (q, cs) = ss.borrow().clone();
            let query = search::SearchQuery::new(&q, cs);
            // 演示：用 rx-preview 过滤示意（真实终端在 P5 接 MessageList）
            if !query.is_empty() {
                st.push_status(state::StatusLevel::Info, format!("搜索：{q} (命中演示)"));
            }
            if let Some(w) = win_weak.upgrade() {
                w.set_search_text(text);
            }
        });
        let win_weak2 = window.as_weak();
        let ss2 = search_state.clone();
        window.on_toggle_search_case(move || {
            let mut s = ss2.borrow_mut();
            s.1 = !s.1;
            if let Some(w) = win_weak2.upgrade() {
                w.set_search_case(s.1);
            }
        });
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

    // TX 模拟：计数 + 回显 + 通知
    {
        let win_weak = window.as_weak();
        let st = app_state.clone();
        window.on_tx_send(move |text| {
            if let Some(w) = win_weak.upgrade() {
                let trimmed = text.trim().to_string();
                let count = w.get_launch_count() + 1;
                w.set_launch_count(count);
                if trimmed.is_empty() {
                    st.push_status(state::StatusLevel::Warn, format!("第 {count} 次：输入为空"));
                    w.set_rx_preview(
                        format!("{}\n[提示 {count}] 输入为空，未发送", w.get_rx_preview()).into(),
                    );
                } else {
                    st.push_status(
                        state::StatusLevel::Info,
                        format!("第 {count} 次发送(模拟)：{trimmed}"),
                    );
                    w.set_rx_preview(
                        format!("{}\n[TX 模拟 {count}] {trimmed}", w.get_rx_preview()).into(),
                    );
                }
                w.set_log_preview(format!("模拟发送 #{count}").into());
            }
        });
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

    // 主题切换（P1：下拉选择内置/自定义主题并持久化）
    {
        let win_weak = window.as_weak();
        let st = app_state.clone();
        window.on_change_theme(move |theme_file| {
            let path = if theme_file.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(theme_file.to_string()))
            };
            if let Some(p) = path.as_deref() {
                if p.exists() {
                    let _ = tool_panels::theme::load_theme_file(p);
                    if let Some(t) = tool_panels::theme::builtin_theme_for_path(p) {
                        tool_panels::theme::set_active_theme(t);
                    }
                    let rel = p
                        .strip_prefix(&st.theme_dir)
                        .unwrap_or(p)
                        .display()
                        .to_string();
                    let mut cfg = st.build_snapshot();
                    cfg.theme_path = Some(rel);
                    let _ = crate::config::save_config_snapshot(&cfg);
                    st.push_status(state::StatusLevel::Info, format!("已切换主题：{}", p.display()));
                }
            }
            if let Some(w) = win_weak.upgrade() {
                theme_bridge::apply_palette_from_panels(&w);
            }
        });
    }

    if app_state.config_migrated {
        let _ = app_state.save();
    }

    window.run()
}

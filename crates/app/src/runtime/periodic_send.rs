use crate::app::WorkbenchApp;
use crate::runtime::timing::wait_until_deadline;
use crate::state::StatusLevel;
use eframe::egui;
use std::sync::{Arc, Mutex};

/// 周期发送后台线程的控制状态。
pub(crate) struct PeriodicSendState {
    /// 取消信号：true 时后台线程应尽快退出。
    pub(crate) cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// 后台线程结束原因（失败/完成），主线程 tick 读取后回写状态栏。
    /// 后台线程无 &mut self，只能通过共享通道传递用户可见反馈。
    pub(crate) outcome: Arc<Mutex<Option<(StatusLevel, String)>>>,
}

impl Default for PeriodicSendState {
    fn default() -> Self {
        Self {
            cancel: None,
            outcome: Arc::new(Mutex::new(None)),
        }
    }
}

impl WorkbenchApp {
    pub(super) fn tick_periodic_send(&mut self, _ctx: &egui::Context) {
        let ps = &mut self.periodic_send;
        // 检查是否被外部关闭，或线程已自然结束（cancel flag 被线程设为 true）
        if ps.cancel.is_some() && !self.send.periodic_enabled {
            if let Some(cancel) = ps.cancel.take() {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            return;
        }
        // 线程已结束（cancel flag 为 true），清理状态并回写用户可见反馈
        if ps
            .cancel
            .as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
        {
            ps.cancel = None;
            self.send.periodic_enabled = false;
            self.send.periodic_send_count = 0;
            // 读取后台线程写入的结束原因（失败/完成），回写状态栏。
            let outcome_msg = ps.outcome.lock().ok().and_then(|mut slot| slot.take());
            if let Some((level, msg)) = outcome_msg {
                self.set_status_force(level, msg);
            }
            return;
        }
        if ps.cancel.is_some() {
            return;
        }
        if !self.send.periodic_enabled {
            return;
        }

        let interval_ms: f64 = match self.send.periodic_interval_ms.trim().parse() {
            Ok(v) if v > 0.0 => v,
            _ => {
                self.send.periodic_enabled = false;
                self.set_status_force(StatusLevel::Warn, "周期发送间隔必须 > 0ms");
                return;
            }
        };
        if !self.send_target_port_open() {
            self.send.periodic_enabled = false;
            self.set_status_force(StatusLevel::Error, "周期发送已停止：目标串口未打开");
            return;
        }
        if self.send.input.is_empty() {
            self.send.periodic_enabled = false;
            self.set_status_force(StatusLevel::Warn, "周期发送已停止：输入为空");
            return;
        }

        // 克隆 spawn 线程需要的所有数据，然后才能通过 ps 写入 cancel。
        // 读取 self 必须在 ps (&mut self.periodic_send) 写入之前完成。
        let port = self.send.target_port.clone().unwrap_or_default();
        let input = self.send.input.clone();
        let hex_mode = self.send.hex_mode;
        let line_ending = self.send.line_ending;
        let hex_strict = self.send.hex_strict;
        let transport = self.workbench.transport_endpoint();
        let max_count = self.send.periodic_max_count;
        let event_sink = self.workbench.event_sink();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let outcome = self.periodic_send.outcome.clone();
        let interval = std::time::Duration::from_secs_f64(interval_ms / 1000.0);

        self.periodic_send.cancel = Some(cancel.clone());

        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let mut count: u64 = 0;

            loop {
                // 基于 start_time + count * interval 计算 absolute deadline
                let deadline = start + interval * (count as u32 + 1);

                // 等到 deadline 或被 cancel：剩余 >2ms 时 sleep 让出 CPU，最后阶段 spin 保精度
                if wait_until_deadline(deadline, Some(&cancel)) {
                    return;
                }

                // 恰好到期，发送
                let err = transport
                    .send(&port, &input, hex_mode, line_ending.suffix(), hex_strict)
                    .err();

                if let Some(e) = err {
                    cancel.store(true, std::sync::atomic::Ordering::Release);
                    let msg = format!("周期发送已在第 {count} 次后停止：{e}");
                    event_sink.publish(tool_application::api::core::Event::system_log(
                        tool_application::api::core::LogLevel::Error,
                        "periodic",
                        msg.clone(),
                    ));
                    if let Ok(mut slot) = outcome.lock() {
                        *slot = Some((StatusLevel::Error, msg));
                    }
                    return;
                }

                count += 1;
                if let Some(max) = max_count
                    && count >= max
                {
                    cancel.store(true, std::sync::atomic::Ordering::Release);
                    let msg = format!("周期发送已完成（{max} 次）");
                    event_sink.publish(tool_application::api::core::Event::system_log(
                        tool_application::api::core::LogLevel::Info,
                        "periodic",
                        msg.clone(),
                    ));
                    if let Ok(mut slot) = outcome.lock() {
                        *slot = Some((StatusLevel::Info, msg));
                    }
                    return;
                }
            }
        });
    }
}

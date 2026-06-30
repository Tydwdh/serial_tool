use crate::app::WorkbenchApp;
use crate::state::StatusLevel;
use eframe::egui;
use tool_transport::send_impl_to;

impl WorkbenchApp {
    pub(super) fn tick_periodic_send(&mut self, _ctx: &egui::Context) {
        // 检查是否被外部关闭，或线程已自然结束（cancel flag 被线程设为 true）
        if self.periodic_send_cancel.is_some() && !self.send.periodic_enabled {
            if let Some(cancel) = self.periodic_send_cancel.take() {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            return;
        }
        // 线程已结束（cancel flag 为 true），清理状态
        if self
            .periodic_send_cancel
            .as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
        {
            self.periodic_send_cancel = None;
            self.send.periodic_enabled = false;
            self.send.periodic_send_count = 0;
            return;
        }
        if self.periodic_send_cancel.is_some() {
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

        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.periodic_send_cancel = Some(cancel.clone());

        let port = self.send.target_port.clone().unwrap_or_default();
        let input = self.send.input.clone();
        let hex_mode = self.send.hex_mode;
        let line_ending = self.send.line_ending;
        let hex_strict = self.send.hex_strict;
        let transport = self.transport.clone();
        let max_count = self.send.periodic_max_count;
        let bus = self.bus.clone();
        let interval = std::time::Duration::from_secs_f64(interval_ms / 1000.0);

        std::thread::spawn(move || {
            // 提升为实时优先级，减少 OS 调度延迟
            #[cfg(target_os = "windows")]
            unsafe {
                unsafe extern "system" {
                    fn SetThreadPriority(thread: isize, priority: i32) -> i32;
                    fn GetCurrentThread() -> isize;
                }
                SetThreadPriority(GetCurrentThread(), 15); // THREAD_PRIORITY_TIME_CRITICAL
            }

            let start = std::time::Instant::now();
            let mut count: u64 = 0;

            loop {
                // 基于 start_time + count * interval 计算 absolute deadline
                let deadline = start + interval * (count as u32 + 1);

                // 纯 spin-wait 到 deadline
                let mut spin_count = 0u32;
                while std::time::Instant::now() < deadline {
                    std::hint::spin_loop();
                    spin_count = spin_count.wrapping_add(1);
                    if spin_count & 0xFF == 0 && cancel.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                }

                // 恰好到期，发送
                let err = send_impl_to(
                    &port,
                    &input,
                    hex_mode,
                    line_ending.suffix(),
                    hex_strict,
                    &transport,
                )
                .err()
                .map(|e| e.to_string());

                if let Some(e) = err {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    bus.publish(tool_core::Event::system_log(
                        tool_core::LogLevel::Error,
                        "periodic",
                        format!("周期发送失败: {e}"),
                    ));
                    return;
                }

                count += 1;
                if let Some(max) = max_count
                    && count >= max
                {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    bus.publish(tool_core::Event::system_log(
                        tool_core::LogLevel::Info,
                        "periodic",
                        format!("周期发送已完成 ({max} 次)"),
                    ));
                    return;
                }
            }
        });
    }
}

//! 拔线/失联后的自动重连调度。
//!
//! 这份状态机取代原先那个「单槽 + 死字段」的待重连状态：原先 `PendingReconnect` 的
//! `attempts` / `next_try_at` 自创建后再无写入点，于是顶栏与设备面板永远显示
//! `0.0s (1/10)`；派发也不是循环（端口回到枚举列表当帧派一次，失败就再也不试），
//! 而且只认 `selected_port` 一个端口。
//!
//! 现在：按端口记账、真退避、有次数上限、能取消。
//!
//! 纯状态机：不读时钟（`now` 由调用方给）、不派发命令、不碰 UI —— 那三件由组合根做，
//! 判定逻辑因此可以在没有 egui、没有串口的测试里逐格驱动。

use std::collections::BTreeMap;

use tool_application::TaskId;

/// 自动重连的最大尝试次数：到这个数就放弃，并明确告诉用户。
pub(crate) const RECONNECT_MAX_ATTEMPTS: u32 = 10;

/// 第 `attempts` 次尝试失败之后等多久再试：0.5s → 1s → 2s → 4s → 8s，之后封顶 8s。
pub(crate) fn reconnect_delay(attempts: u32) -> f64 {
    (0.5 * 2_f64.powi(attempts.min(4) as i32)).min(8.0)
}

/// 一个端口的待重连状态。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReconnectEntry {
    pub(crate) port: String,
    /// 已经失败的尝试次数（不含正在飞的那一次）。
    pub(crate) attempts: u32,
    /// 下一次尝试的时间（秒，来自 `ctx.input().time`）。
    pub(crate) next_try_at: f64,
    /// 已派发、还没定终态的那次尝试。
    pub(crate) task_id: Option<TaskId>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ReconnectOutcome {
    Retry { attempts: u32, delay: f64 },
    GaveUp { attempts: u32 },
}

/// 待重连端口表。
#[derive(Default)]
pub(crate) struct ReconnectTracker {
    entries: BTreeMap<String, ReconnectEntry>,
}

impl ReconnectTracker {
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn entry(&self, port: &str) -> Option<&ReconnectEntry> {
        self.entries.get(port)
    }

    /// 挂起一个端口，首次尝试立即到期。已在表里就不动它 —— 端口列表每次刷新都会调这里，
    /// 顺带重置退避会让"重试间隔"永远停在第一档。
    pub(crate) fn schedule(&mut self, port: &str, now: f64) -> bool {
        if self.entries.contains_key(port) {
            return false;
        }
        self.entries.insert(
            port.to_owned(),
            ReconnectEntry {
                port: port.to_owned(),
                attempts: 0,
                next_try_at: now,
                task_id: None,
            },
        );
        true
    }

    /// 取消（用户点了取消、关闭了这个端口、或关掉了自动重连）。
    pub(crate) fn cancel(&mut self, port: &str) -> Option<ReconnectEntry> {
        self.entries.remove(port)
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// 已派发、还没定终态的尝试（端口 → 任务号）。
    pub(crate) fn in_flight(&self) -> impl Iterator<Item = (&str, TaskId)> {
        self.entries
            .values()
            .filter_map(|entry| entry.task_id.map(|id| (entry.port.as_str(), id)))
    }

    /// 界面上优先展示的那一条：选中的端口优先，否则按端口名取第一个（与端口列表同序）。
    pub(crate) fn display(&self, selected: Option<&str>) -> Option<&ReconnectEntry> {
        selected
            .and_then(|port| self.entries.get(port))
            .or_else(|| self.entries.values().next())
    }

    /// 到点、且没有在飞任务的端口 —— 这些该派发重连。
    pub(crate) fn take_due(&self, now: f64) -> Vec<String> {
        self.entries
            .values()
            .filter(|entry| entry.task_id.is_none() && entry.next_try_at <= now)
            .map(|entry| entry.port.clone())
            .collect()
    }

    pub(crate) fn mark_dispatched(&mut self, port: &str, task_id: TaskId) {
        if let Some(entry) = self.entries.get_mut(port) {
            entry.task_id = Some(task_id);
        }
    }

    /// 这次尝试没成：记一次、安排下一次；次数用尽则放弃（从表里摘掉，由调用方报给用户）。
    pub(crate) fn record_attempt_failed(
        &mut self,
        port: &str,
        now: f64,
    ) -> Option<ReconnectOutcome> {
        let entry = self.entries.get_mut(port)?;
        entry.task_id = None;
        entry.attempts = entry.attempts.saturating_add(1);
        if entry.attempts >= RECONNECT_MAX_ATTEMPTS {
            let attempts = entry.attempts;
            self.entries.remove(port);
            return Some(ReconnectOutcome::GaveUp { attempts });
        }
        let delay = reconnect_delay(entry.attempts);
        entry.next_try_at = now + delay;
        Some(ReconnectOutcome::Retry {
            attempts: entry.attempts,
            delay,
        })
    }

    /// 这次尝试成功：该端口不再需要重连。
    pub(crate) fn record_success(&mut self, port: &str) -> bool {
        self.entries.remove(port).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(port: &str) -> String {
        port.to_owned()
    }

    fn task(id: u64) -> TaskId {
        TaskId(id)
    }

    #[test]
    fn schedule_does_not_reset_an_entry_that_is_already_waiting() {
        let mut tracker = ReconnectTracker::default();
        assert!(tracker.schedule("COM3", 100.0));
        let entry = tracker.entry("COM3").cloned().expect("已挂起");
        // 端口列表每 0.5s 刷新一次，每次都会走 schedule：不能把退避清零。
        tracker
            .record_attempt_failed("COM3", 200.0)
            .expect("记一次失败");
        assert!(!tracker.schedule("COM3", 300.0), "已在表里就不该再挂一次");
        let waiting = tracker.entry("COM3").expect("还在表里");
        assert_eq!(waiting.attempts, 1, "重复 schedule 不得重置尝试次数");
        assert_eq!(waiting.next_try_at, 200.0 + reconnect_delay(1));
        assert_eq!(entry.port, "COM3");
    }

    #[test]
    fn backoff_ladder_matches_the_documented_steps_and_caps_at_eight_seconds() {
        let ladder: Vec<f64> = (1..=RECONNECT_MAX_ATTEMPTS).map(reconnect_delay).collect();
        assert_eq!(
            ladder,
            vec![1.0, 2.0, 4.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0, 8.0]
        );
        assert_eq!(reconnect_delay(0), 0.5, "第 0 档（首败之前）是 0.5s");
    }

    #[test]
    fn tracker_gives_up_after_the_attempt_cap_and_drops_the_entry() {
        let mut tracker = ReconnectTracker::default();
        tracker.schedule("COM3", 0.0);
        for attempt in 1..RECONNECT_MAX_ATTEMPTS {
            let outcome = tracker
                .record_attempt_failed("COM3", f64::from(attempt))
                .expect("还在重试");
            assert_eq!(
                outcome,
                ReconnectOutcome::Retry {
                    attempts: attempt,
                    delay: reconnect_delay(attempt),
                }
            );
            assert_eq!(tracker.len(), 1);
        }
        assert_eq!(
            tracker.record_attempt_failed("COM3", 100.0),
            Some(ReconnectOutcome::GaveUp {
                attempts: RECONNECT_MAX_ATTEMPTS
            }),
            "第 {RECONNECT_MAX_ATTEMPTS} 次失败必须放弃"
        );
        assert!(tracker.is_empty(), "放弃后不该留下悬挂条目");
        assert_eq!(tracker.entry("COM3"), None);
    }

    #[test]
    fn only_ports_that_are_due_and_not_in_flight_are_dispatched() {
        let mut tracker = ReconnectTracker::default();
        tracker.schedule("COM3", 10.0);
        tracker.schedule("COM4", 10.0);

        assert_eq!(tracker.take_due(9.9), Vec::<String>::new(), "没到点不派发");
        assert_eq!(tracker.take_due(10.0), vec![s("COM3"), s("COM4")]);
        tracker.mark_dispatched("COM3", task(7));
        assert_eq!(
            tracker.take_due(1_000.0),
            vec![s("COM4")],
            "在飞的那次还没定终态，不能重复派发"
        );
        assert_eq!(tracker.entry("COM3").unwrap().task_id, Some(task(7)));
    }

    #[test]
    fn display_prefers_the_selected_port_and_success_removes_the_entry() {
        let mut tracker = ReconnectTracker::default();
        tracker.schedule("COM3", 0.0);
        tracker.schedule("COM7", 0.0);
        assert_eq!(
            tracker
                .display(Some("COM7"))
                .map(|entry| entry.port.as_str()),
            Some("COM7"),
            "选中的端口优先展示"
        );
        assert_eq!(
            tracker
                .display(Some("COM9"))
                .map(|entry| entry.port.as_str()),
            Some("COM3"),
            "选中端口不在重连表里时按端口名取第一个"
        );
        assert!(tracker.record_success("COM3"), "成功要摘掉条目");
        assert_eq!(tracker.len(), 1);
        assert!(tracker.cancel("COM7").is_some(), "取消要摘掉条目");
        assert!(tracker.is_empty());
    }
}

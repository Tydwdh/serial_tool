//! 共用的回放策略选择器。
//!
//! 载入文件与执行 analyzer 仍归平台/应用层负责；策略选择器本身刻意做得很小、
//! 且不依赖平台，这样 Native 与 Web 的回放面板不会在视觉和语义上各自漂移。

use crate::theme;
use serde::{Deserialize, Serialize};

/// 用户可选的回放策略，两个平台共用同一份定义与文案。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ReplayPolicyOption {
    #[default]
    AutoPreferRecorded,
    ExactRecorded,
    ReparseRaw,
}

impl ReplayPolicyOption {
    pub const ALL: [Self; 3] = [
        Self::AutoPreferRecorded,
        Self::ExactRecorded,
        Self::ReparseRaw,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::AutoPreferRecorded => "自动",
            Self::ExactRecorded => "使用录制解析结果",
            Self::ReparseRaw => "重新解析原始串口",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::AutoPreferRecorded => "有已录制的解析事件时优先使用，否则重新解析原始串口",
            Self::ExactRecorded => "只使用录制时保存的 protocol.* 事件",
            Self::ReparseRaw => "使用 Replay Analyzer 从原始串口事件重新生成协议事件",
        }
    }
}

/// 渲染共用的策略选择器；返回本帧内选中策略是否发生变化。
pub fn replay_policy_ui(
    ui: &mut egui::Ui,
    policy: &mut ReplayPolicyOption,
    effective: Option<ReplayPolicyOption>,
) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label("回放策略");
        egui::ComboBox::from_id_salt("shared-replay-policy")
            .width(190.0)
            .selected_text(policy.label())
            .show_ui(ui, |ui| {
                for candidate in ReplayPolicyOption::ALL {
                    if ui
                        .selectable_value(policy, candidate, candidate.label())
                        .on_hover_text(candidate.description())
                        .changed()
                    {
                        changed = true;
                    }
                }
            });
    });

    let effective = effective.unwrap_or(*policy);
    let effective_text = match effective {
        ReplayPolicyOption::AutoPreferRecorded => "自动（按录制内容选择）",
        ReplayPolicyOption::ExactRecorded => "使用录制解析结果",
        ReplayPolicyOption::ReparseRaw => "使用 Replay Analyzer 重新解析",
    };
    ui.label(egui::RichText::new(effective_text).color(theme::text_secondary()));
    changed
}

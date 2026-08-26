//! Shared replay-policy presentation.
//!
//! Loading and analyzer execution remain platform/application concerns. The
//! policy selector itself is deliberately small and platform-neutral so the
//! Native and Web replay panels cannot drift visually or semantically.

use crate::theme;
use serde::{Deserialize, Serialize};

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

/// Render the shared policy selector. Returns whether the selected policy
/// changed during this frame.
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

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Activity {
    #[default]
    Devices,
    Terminal,
    Plugins,
    Logs,
    Settings,
    Replay,
}

impl Activity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Devices => "设备",
            Self::Terminal => "终端",
            Self::Plugins => "插件",
            Self::Logs => "日志",
            Self::Settings => "设置",
            Self::Replay => "回放",
        }
    }

    pub fn panel_kind(self) -> Option<PanelKind> {
        match self {
            Self::Replay => Some(PanelKind::Replay),
            Self::Terminal => Some(PanelKind::Terminal),
            Self::Logs => Some(PanelKind::Logs),
            Self::Devices | Self::Plugins | Self::Settings => None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PanelKind {
    #[default]
    Replay,
    Terminal,
    Logs,
    Dynamic(String),
}

impl PanelKind {
    pub fn title(&self) -> String {
        match self {
            Self::Replay => "回放".to_owned(),
            Self::Terminal => "终端".to_owned(),
            Self::Logs => "日志".to_owned(),
            Self::Dynamic(id) => id.clone(),
        }
    }

    pub fn activity(&self) -> Option<Activity> {
        match self {
            Self::Replay => Some(Activity::Replay),
            Self::Terminal => Some(Activity::Terminal),
            Self::Logs => Some(Activity::Logs),
            Self::Dynamic(_) => None,
        }
    }

    pub fn dynamic_id(&self) -> Option<&str> {
        match self {
            Self::Dynamic(id) => Some(id),
            _ => None,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PanelManager {
    pub activity: Activity,
    pub tabs: Vec<PanelKind>,
    pub active_tab: PanelKind,
    pub inspector_visible: bool,
    #[serde(default = "default_true")]
    pub bottom_logs_visible: bool,
}

impl PanelManager {
    pub fn select_activity(&mut self, activity: Activity) {
        self.activity = activity;
        if let Some(kind) = activity.panel_kind() {
            self.active_tab = kind;
        } else if self.active_tab.dynamic_id().is_some() {
            self.active_tab = PanelKind::Replay;
        }
    }

    pub fn open_tab(&mut self, kind: PanelKind) {
        if !self.tabs.contains(&kind) {
            self.tabs.push(kind.clone());
        }
        if let Some(activity) = kind.activity() {
            self.activity = activity;
        }
        self.active_tab = kind;
    }

    /// 添加标签但不自动切换（插件后台创建面板时使用）
    pub fn add_tab(&mut self, kind: PanelKind) {
        if !self.tabs.contains(&kind) {
            self.tabs.push(kind);
        }
    }

    pub fn close_tab(&mut self, kind: PanelKind) {
        if self.active_tab == kind {
            self.active_tab = self
                .tabs
                .iter()
                .rev()
                .find(|candidate| *candidate != &kind)
                .cloned()
                .or_else(|| self.activity.panel_kind())
                .unwrap_or_default();
            if let Some(activity) = self.active_tab.activity() {
                self.activity = activity;
            }
        }
        self.tabs.retain(|k| k != &kind);
    }

    pub fn active_dynamic_id(&self) -> Option<&str> {
        self.active_tab.dynamic_id()
    }

    pub fn discard_dynamic_tabs(&mut self) {
        self.tabs.retain(|kind| kind.dynamic_id().is_none());
        if self.active_tab.dynamic_id().is_some() {
            self.active_tab = self.activity.panel_kind().unwrap_or_default();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_activity_leaves_dynamic_tab() {
        let mut manager = PanelManager::default();
        manager.open_tab(PanelKind::Dynamic("pid-chart".to_owned()));

        manager.select_activity(Activity::Devices);

        assert_eq!(manager.activity, Activity::Devices);
        assert!(manager.active_dynamic_id().is_none());
    }

    #[test]
    fn closing_active_dynamic_falls_back_to_previous_dynamic() {
        let mut manager = PanelManager::default();
        let first = PanelKind::Dynamic("a".to_owned());
        let second = PanelKind::Dynamic("b".to_owned());
        manager.open_tab(first.clone());
        manager.open_tab(second.clone());

        manager.close_tab(second);

        assert_eq!(manager.active_tab, first);
    }

    #[test]
    fn discard_dynamic_tabs_removes_transient_state() {
        let mut manager = PanelManager::default();
        manager.open_tab(PanelKind::Dynamic("runtime".to_owned()));

        manager.discard_dynamic_tabs();

        assert!(manager.tabs.is_empty());
        assert!(manager.active_dynamic_id().is_none());
    }
}

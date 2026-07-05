use serde::{Deserialize, Serialize};

/// 面板种类。左侧栏（= Center 标签栏）、底部、右侧三个停靠区共用同一套 PanelKind。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PanelKind {
    #[default]
    Devices,
    Replay,
    Plugins,
    Settings,
    Terminal,
    Sender,
    Logs,
    Dynamic(String),
}

impl PanelKind {
    pub fn title(&self) -> String {
        match self {
            Self::Devices => "设备".to_owned(),
            Self::Replay => "回放".to_owned(),
            Self::Plugins => "插件".to_owned(),
            Self::Settings => "设置".to_owned(),
            Self::Terminal => "接收".to_owned(),
            Self::Sender => "发送器".to_owned(),
            Self::Logs => "日志".to_owned(),
            Self::Dynamic(id) => id.clone(),
        }
    }

    /// 左侧栏/标签栏显示用的图标（emoji）。
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Devices => "📟",
            Self::Replay => "⏪",
            Self::Plugins => "🧩",
            Self::Settings => "⚙",
            Self::Terminal => "📡",
            Self::Sender => "📤",
            Self::Logs => "📝",
            Self::Dynamic(_) => "🔌",
        }
    }

    pub fn dynamic_id(&self) -> Option<&str> {
        match self {
            Self::Dynamic(id) => Some(id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DockArea {
    Center,
    Bottom,
    Right,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DockStack {
    pub tabs: Vec<PanelKind>,
    pub active: Option<PanelKind>,
}

impl DockStack {
    pub fn open(&mut self, kind: PanelKind) {
        if !self.tabs.contains(&kind) {
            self.tabs.push(kind.clone());
        }
        self.active = Some(kind);
    }

    /// 添加标签但不切换焦点（后台创建面板时使用）
    pub fn add_inactive(&mut self, kind: PanelKind) {
        if !self.tabs.contains(&kind) {
            self.tabs.push(kind.clone());
        }
        if self.active.is_none() {
            self.active = Some(kind);
        }
    }

    pub fn close(&mut self, kind: &PanelKind) {
        // 找到关闭位置，优先选择相邻的 tab 而非最后一个
        let closed_pos = self.tabs.iter().position(|tab| tab == kind);
        self.tabs.retain(|tab| tab != kind);
        if self.active.as_ref() == Some(kind) {
            // 选择紧邻的右侧tab，如果不存在则选择左侧
            let pos = closed_pos.unwrap_or(0);
            self.active = self
                .tabs
                .get(pos)
                .or_else(|| self.tabs.get(pos.saturating_sub(1)))
                .cloned();
        }
    }

    pub fn remove(&mut self, kind: &PanelKind) -> bool {
        let old_len = self.tabs.len();
        self.close(kind);
        old_len != self.tabs.len()
    }

    pub fn contains(&self, kind: &PanelKind) -> bool {
        self.tabs.contains(kind)
    }

    pub fn active_or_first(&self) -> Option<PanelKind> {
        self.active
            .clone()
            .filter(|kind| self.tabs.contains(kind))
            .or_else(|| self.tabs.first().cloned())
    }

    pub(crate) fn discard_dynamic_tabs(&mut self) {
        self.tabs.retain(|kind| kind.dynamic_id().is_none());
        if self
            .active
            .as_ref()
            .is_some_and(|kind| kind.dynamic_id().is_some())
        {
            self.active = self.tabs.last().cloned();
        }
    }

    pub fn reorder(&mut self, kind: &PanelKind, mut insert_index: usize) -> bool {
        let Some(source_index) = self.tabs.iter().position(|tab| tab == kind) else {
            return false;
        };

        insert_index = insert_index.min(self.tabs.len());

        if insert_index > source_index {
            insert_index -= 1;
        }

        if insert_index == source_index {
            return false;
        }

        let item = self.tabs.remove(source_index);
        let insert_index = insert_index.min(self.tabs.len());
        self.tabs.insert(insert_index, item);

        true
    }
}

fn default_true() -> bool {
    true
}

fn default_bottom_size() -> f32 {
    420.0
}

fn default_right_size() -> f32 {
    320.0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DockLayout {
    #[serde(default = "default_true")]
    pub activity_bar_visible: bool,
    #[serde(default = "default_true")]
    pub bottom_visible: bool,
    #[serde(default)]
    pub right_visible: bool,
    #[serde(default = "default_bottom_size")]
    pub bottom_size: f32,
    #[serde(default = "default_right_size")]
    pub right_size: f32,
    #[serde(default)]
    pub center: DockStack,
    #[serde(default)]
    pub bottom: DockStack,
    #[serde(default)]
    pub right: DockStack,
}

impl Default for DockLayout {
    fn default() -> Self {
        let mut center = DockStack::default();
        center.open(PanelKind::Devices);
        center.open(PanelKind::Replay);
        center.open(PanelKind::Plugins);
        center.open(PanelKind::Settings);

        let mut bottom = DockStack::default();
        bottom.open(PanelKind::Terminal);
        bottom.open(PanelKind::Logs);
        bottom.open(PanelKind::Sender);
        bottom.active = Some(PanelKind::Terminal);

        Self {
            activity_bar_visible: true,
            bottom_visible: true,
            right_visible: false,
            bottom_size: default_bottom_size(),
            right_size: default_right_size(),
            center,
            bottom,
            right: DockStack::default(),
        }
    }
}

impl DockLayout {
    pub fn stack_mut(&mut self, area: DockArea) -> &mut DockStack {
        match area {
            DockArea::Center => &mut self.center,
            DockArea::Bottom => &mut self.bottom,
            DockArea::Right => &mut self.right,
        }
    }

    pub fn stack(&self, area: DockArea) -> &DockStack {
        match area {
            DockArea::Center => &self.center,
            DockArea::Bottom => &self.bottom,
            DockArea::Right => &self.right,
        }
    }

    pub fn move_panel(&mut self, kind: PanelKind, to: DockArea) {
        self.center.remove(&kind);
        self.bottom.remove(&kind);
        self.right.remove(&kind);
        self.stack_mut(to).open(kind);

        match to {
            DockArea::Bottom => self.bottom_visible = true,
            DockArea::Right => self.right_visible = true,
            DockArea::Center => {}
        }
    }

    /// 移动面板到目标停靠区的指定位置。
    pub fn insert_panel_at(&mut self, kind: PanelKind, to: DockArea, index: usize) {
        self.center.remove(&kind);
        self.bottom.remove(&kind);
        self.right.remove(&kind);
        let stack = self.stack_mut(to);
        let idx = index.min(stack.tabs.len());
        stack.tabs.insert(idx, kind.clone());
        stack.active = Some(kind);

        match to {
            DockArea::Bottom => self.bottom_visible = true,
            DockArea::Right => self.right_visible = true,
            DockArea::Center => {}
        }
    }

    pub fn all_tabs(&self) -> Vec<PanelKind> {
        self.center
            .tabs
            .iter()
            .chain(self.bottom.tabs.iter())
            .chain(self.right.tabs.iter())
            .cloned()
            .collect()
    }

    fn discard_dynamic_tabs(&mut self) {
        self.center.discard_dynamic_tabs();
        self.bottom.discard_dynamic_tabs();
        self.right.discard_dynamic_tabs();
    }

    pub fn normalize_tool_layout(&mut self) {
        // 工具面板不允许留在 Center：如果被放在 Center，移到底部
        for kind in [PanelKind::Terminal, PanelKind::Logs] {
            if self.center.remove(&kind) {
                if !self.bottom.contains(&kind) && !self.right.contains(&kind) {
                    self.bottom.open(kind);
                }
                self.bottom_visible = true;
            }
        }

        // Sender 不允许在 Center stack 中存在
        self.center.remove(&PanelKind::Sender);

        // Sender 如果没有在任何区域，默认放到底部
        if !self.bottom.contains(&PanelKind::Sender) && !self.right.contains(&PanelKind::Sender) {
            self.bottom.open(PanelKind::Sender);
        }

        // 确保功能面板（回放、插件、设置）至少在一个停靠区中存在。
        // 避免旧版配置文件或异常重置后用户看不到这些核心功能入口。
        for kind in [PanelKind::Replay, PanelKind::Plugins, PanelKind::Settings] {
            if !self.center.contains(&kind)
                && !self.bottom.contains(&kind)
                && !self.right.contains(&kind)
            {
                self.center.open(kind);
            }
        }

        if self.bottom.active.is_none()
            || self
                .bottom
                .active
                .as_ref()
                .is_none_or(|k| !self.bottom.contains(k))
        {
            self.bottom.active = self.bottom.tabs.first().cloned();
        }
        if self.right.active.is_none()
            || self
                .right
                .active
                .as_ref()
                .is_none_or(|k| !self.right.contains(k))
        {
            self.right.active = self.right.tabs.first().cloned();
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PanelManager {
    pub active_tab: PanelKind,
    #[serde(default)]
    pub inspector_visible: bool, // legacy: ignored, kept for old workspace compatibility
    #[serde(default)]
    pub dock: DockLayout,
}

impl Default for PanelManager {
    fn default() -> Self {
        Self {
            active_tab: PanelKind::Devices,
            inspector_visible: false,
            dock: DockLayout::default(),
        }
    }
}

impl PanelManager {
    /// 从 dock 的所有 stack 中派生 tabs 列表（唯一真相来源）
    pub fn tabs(&self) -> Vec<PanelKind> {
        self.dock.all_tabs()
    }

    /// 切换 Center 标签栏的激活面板（左侧栏点击）。
    pub fn select_center_panel(&mut self, kind: PanelKind) {
        self.active_tab = kind.clone();
        self.dock.center.active = Some(kind);
    }

    pub fn open_tab(&mut self, kind: PanelKind) {
        self.active_tab = kind.clone();
        self.dock.center.open(kind);
    }

    /// 同步 active_tab 到 Center 当前激活面板（Center 渲染后调用）。
    pub fn sync_active_tab_from_center(&mut self) {
        if let Some(kind) = self.dock.center.active_or_first() {
            self.active_tab = kind;
        } else {
            self.active_tab = self.dock.all_tabs().first().cloned().unwrap_or_default();
        }
    }

    pub fn is_panel_visible(&self, kind: &PanelKind) -> bool {
        self.dock.center.active_or_first().as_ref() == Some(kind)
            || (self.dock.bottom_visible
                && self.dock.bottom.active_or_first().as_ref() == Some(kind))
            || (self.dock.right_visible && self.dock.right.active_or_first().as_ref() == Some(kind))
    }

    /// 添加标签但不自动切换（插件后台创建面板时使用）
    pub fn add_tab(&mut self, kind: PanelKind) {
        self.dock.center.add_inactive(kind);
    }

    pub fn close_tab(&mut self, kind: PanelKind) {
        // 确定关闭的 tab 在哪个 dock area，在该 stack 中查找回退
        let area = if self.dock.center.contains(&kind) {
            DockArea::Center
        } else if self.dock.bottom.contains(&kind) {
            DockArea::Bottom
        } else if self.dock.right.contains(&kind) {
            DockArea::Right
        } else {
            return; // tab 不在任何 dock 中
        };

        if self.active_tab == kind {
            // 在同一 dock stack 中查找最近的非关闭 tab
            let stack_tabs = &self.dock.stack(area).tabs;
            self.active_tab = stack_tabs
                .iter()
                .rev()
                .find(|candidate| *candidate != &kind)
                .cloned()
                .or_else(|| self.dock.all_tabs().first().cloned())
                .unwrap_or_default();
        }
        self.dock.center.remove(&kind);
        self.dock.bottom.remove(&kind);
        self.dock.right.remove(&kind);
    }

    pub fn active_dynamic_id(&self) -> Option<&str> {
        self.active_tab.dynamic_id()
    }

    pub fn discard_dynamic_tabs(&mut self) {
        if self.active_tab.dynamic_id().is_some() {
            self.active_tab = self
                .dock
                .center
                .tabs
                .iter()
                .find(|k| k.dynamic_id().is_none())
                .cloned()
                .unwrap_or_default();
        }
        self.dock.discard_dynamic_tabs();
    }

    /// 在 dock.move_panel() 调用后同步 active_tab
    pub fn sync_tabs_from_dock(&mut self) {
        let all_tabs = self.dock.all_tabs();
        if !all_tabs.contains(&self.active_tab) {
            self.active_tab = all_tabs.first().cloned().unwrap_or_default();
        }
        self.sync_active_tab_from_center();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_center_panel_switches_active() {
        let mut manager = PanelManager::default();
        manager.dock.center.open(PanelKind::Replay);

        manager.select_center_panel(PanelKind::Replay);

        assert_eq!(manager.active_tab, PanelKind::Replay);
        assert_eq!(manager.dock.center.active, Some(PanelKind::Replay));
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

        assert!(!manager.tabs().iter().any(|k| k.dynamic_id().is_some()));
        assert!(manager.active_dynamic_id().is_none());
    }

    #[test]
    fn sync_tabs_from_dock_uses_center_active_panel() {
        let mut manager = PanelManager {
            active_tab: PanelKind::Devices,
            ..Default::default()
        };
        manager.dock.center.open(PanelKind::Settings);

        manager.sync_tabs_from_dock();

        assert_eq!(manager.active_tab, PanelKind::Settings);
    }

    #[test]
    fn is_panel_visible_checks_active_dock_stacks() {
        let mut manager = PanelManager::default();
        manager.dock.center.open(PanelKind::Settings);
        manager.dock.bottom_visible = true;
        manager.dock.bottom.open(PanelKind::Logs);
        manager.dock.right_visible = false;
        manager.dock.right.open(PanelKind::Terminal);

        assert!(manager.is_panel_visible(&PanelKind::Settings));
        assert!(manager.is_panel_visible(&PanelKind::Logs));
        assert!(!manager.is_panel_visible(&PanelKind::Terminal));
    }
}

use egui_tiles::{Container, Tile, TileId, Tiles, Tree};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

/// egui_tiles 的持久化布局。额外保存三个默认 tab 容器的 id，
/// 使顶部工具栏和快捷键仍能快速显示/隐藏底部、右侧区域。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TilesLayout {
    pub tree: Tree<PanelKind>,
    pub main_tabs: TileId,
    pub bottom_tabs: TileId,
    pub right_tabs: TileId,
    /// 动态面板的所属插件，仅在运行期分组时使用。
    #[serde(default)]
    pub plugin_panel_owners: BTreeMap<String, String>,
    /// 一个插件有多个动态面板时，对应的父标签容器。
    #[serde(default)]
    pub plugin_groups: BTreeMap<String, TileId>,
}

impl TilesLayout {
    fn pane_ids(tiles: &mut Tiles<PanelKind>, panes: &[PanelKind]) -> Vec<TileId> {
        panes
            .iter()
            .cloned()
            .map(|pane| tiles.insert_pane(pane))
            .collect()
    }

    /// 将 v0.7.3 的三栏 Dock 配置转换为可自由拆分、拖拽的 tiles 树。
    pub fn from_legacy(dock: &DockLayout) -> Self {
        let mut tiles = Tiles::default();

        let center = if dock.center.tabs.is_empty() {
            vec![PanelKind::Devices]
        } else {
            dock.center.tabs.clone()
        };
        let bottom = if dock.bottom.tabs.is_empty() {
            vec![PanelKind::Terminal, PanelKind::Logs, PanelKind::Sender]
        } else {
            dock.bottom.tabs.clone()
        };

        let main_panes = Self::pane_ids(&mut tiles, &center);
        let bottom_panes = Self::pane_ids(&mut tiles, &bottom);
        let right_panes = Self::pane_ids(&mut tiles, &dock.right.tabs);
        let main_tabs = tiles.insert_tab_tile(main_panes);
        let bottom_tabs = tiles.insert_tab_tile(bottom_panes);
        let right_tabs = tiles.insert_tab_tile(right_panes);

        if let Some(active) = dock.center.active.as_ref()
            && let Some(id) = tiles.find_pane(active)
            && let Some(Tile::Container(Container::Tabs(tabs))) = tiles.get_mut(main_tabs)
        {
            tabs.set_active(id);
        }
        if let Some(active) = dock.bottom.active.as_ref()
            && let Some(id) = tiles.find_pane(active)
            && let Some(Tile::Container(Container::Tabs(tabs))) = tiles.get_mut(bottom_tabs)
        {
            tabs.set_active(id);
        }
        if let Some(active) = dock.right.active.as_ref()
            && let Some(id) = tiles.find_pane(active)
            && let Some(Tile::Container(Container::Tabs(tabs))) = tiles.get_mut(right_tabs)
        {
            tabs.set_active(id);
        }

        let main_column = tiles.insert_vertical_tile(vec![main_tabs, bottom_tabs]);
        let root = tiles.insert_horizontal_tile(vec![main_column, right_tabs]);
        let mut tree = Tree::new("hardware-workbench-layout", root, tiles);
        tree.set_visible(bottom_tabs, dock.bottom_visible);
        tree.set_visible(
            right_tabs,
            dock.right_visible && !dock.right.tabs.is_empty(),
        );

        Self {
            tree,
            main_tabs,
            bottom_tabs,
            right_tabs,
            plugin_panel_owners: BTreeMap::new(),
            plugin_groups: BTreeMap::new(),
        }
    }

    pub fn set_bottom_visible(&mut self, visible: bool) {
        if self.tree.tiles.get(self.bottom_tabs).is_some() {
            self.tree.set_visible(self.bottom_tabs, visible);
        }
    }

    pub fn bottom_visible(&self) -> bool {
        self.tree.tiles.get(self.bottom_tabs).is_some() && self.tree.is_visible(self.bottom_tabs)
    }

    pub fn set_right_visible(&mut self, visible: bool) {
        if self.tree.tiles.get(self.right_tabs).is_some() {
            self.tree.set_visible(self.right_tabs, visible);
        }
    }

    pub fn right_visible(&self) -> bool {
        self.tree.tiles.get(self.right_tabs).is_some() && self.tree.is_visible(self.right_tabs)
    }

    pub fn select_pane(&mut self, kind: &PanelKind) -> bool {
        let Some(id) = self.tree.tiles.find_pane(kind) else {
            return false;
        };
        let mut child = id;
        let mut selected = false;
        while let Some(parent) = self.tree.tiles.parent_of(child) {
            if let Some(Tile::Container(Container::Tabs(tabs))) = self.tree.tiles.get_mut(parent) {
                tabs.set_active(child);
                selected = true;
            }
            child = parent;
        }
        selected
    }

    pub fn add_to_main_tabs(&mut self, kind: PanelKind, activate: bool) {
        if let Some(id) = self.tree.tiles.find_pane(&kind) {
            if activate {
                self.select_pane(&kind);
            }
            self.tree.set_visible(id, true);
            return;
        }

        let main_tabs = self.ensure_main_tabs();
        let id = self.tree.tiles.insert_pane(kind);
        if let Some(Tile::Container(Container::Tabs(tabs))) = self.tree.tiles.get_mut(main_tabs) {
            tabs.add_child(id);
            if activate {
                tabs.set_active(id);
            }
        }
    }

    /// 打开由插件拥有的动态面板。第一个面板保持独立；同一插件创建第二个
    /// 面板时，自动建立父标签容器，将两个面板作为子标签收纳其中。
    pub fn add_to_plugin_tabs(&mut self, kind: PanelKind, plugin_id: &str) {
        let Some(panel_id) = kind.dynamic_id().map(str::to_owned) else {
            self.add_to_main_tabs(kind, true);
            return;
        };

        if self.tree.tiles.find_pane(&kind).is_some() {
            self.plugin_panel_owners
                .insert(panel_id, plugin_id.to_owned());
            self.select_pane(&kind);
            return;
        }

        let group_id = self.plugin_groups.get(plugin_id).copied().filter(|id| {
            matches!(
                self.tree.tiles.get(*id),
                Some(Tile::Container(Container::Tabs(_)))
            )
        });
        if group_id.is_none() {
            self.plugin_groups.remove(plugin_id);
        }

        let existing_pane = self
            .plugin_panel_owners
            .iter()
            .find(|(_, owner)| owner.as_str() == plugin_id)
            .and_then(|(id, _)| self.tree.tiles.find_pane(&PanelKind::Dynamic(id.clone())));

        let new_pane = self.tree.tiles.insert_pane(kind.clone());
        self.plugin_panel_owners
            .insert(panel_id, plugin_id.to_owned());

        if let Some(group_id) = group_id {
            if let Some(Tile::Container(Container::Tabs(tabs))) = self.tree.tiles.get_mut(group_id)
            {
                tabs.add_child(new_pane);
                tabs.set_active(new_pane);
            }
            self.select_pane(&kind);
            return;
        }

        let Some(existing_pane) = existing_pane else {
            // 插件的第一个面板：作为普通独立窗格插入。
            let main_tabs = self.ensure_main_tabs();
            if let Some(Tile::Container(Container::Tabs(tabs))) = self.tree.tiles.get_mut(main_tabs)
            {
                tabs.add_child(new_pane);
                tabs.set_active(new_pane);
            }
            self.select_pane(&kind);
            return;
        };

        // 必须在插入新 Tabs 容器前记录旧父节点。插入后 `existing_pane` 同时属于
        // 旧容器和新容器，`parent_of` 的返回顺序不保证；错误地取到新容器会把它
        // 加回自身，形成循环布局树并在渲染时栈溢出。
        let existing_parent = self.tree.tiles.parent_of(existing_pane);
        let group_id = self
            .tree
            .tiles
            .insert_tab_tile(vec![existing_pane, new_pane]);
        if let Some(parent_id) = existing_parent {
            if let Some(Tile::Container(parent)) = self.tree.tiles.get_mut(parent_id) {
                parent.remove_child(existing_pane);
                parent.add_child(group_id);
                if let Container::Tabs(tabs) = parent {
                    tabs.set_active(group_id);
                }
            }
        } else {
            self.tree.root = Some(group_id);
        }
        self.plugin_groups.insert(plugin_id.to_owned(), group_id);
        self.select_pane(&kind);
    }

    /// 返回可承载新标签的主标签栏。布局简化可能回收原来的容器，
    /// 此时优先复用当前可见标签栏，确保运行时新建面板不会成为孤立节点。
    fn ensure_main_tabs(&mut self) -> TileId {
        if matches!(
            self.tree.tiles.get(self.main_tabs),
            Some(Tile::Container(Container::Tabs(_)))
        ) {
            return self.main_tabs;
        }

        if let Some(existing_tabs) = self.tree.active_tiles().into_iter().rev().find(|id| {
            matches!(
                self.tree.tiles.get(*id),
                Some(Tile::Container(Container::Tabs(_)))
            )
        }) {
            self.main_tabs = existing_tabs;
            return existing_tabs;
        }

        let main_tabs = self.tree.tiles.insert_tab_tile(Vec::new());
        if let Some(root) = self
            .tree
            .root
            .filter(|id| self.tree.tiles.get(*id).is_some())
        {
            let new_root = self
                .tree
                .tiles
                .insert_horizontal_tile(vec![root, main_tabs]);
            self.tree.root = Some(new_root);
        } else {
            self.tree.root = Some(main_tabs);
        }
        self.main_tabs = main_tabs;
        main_tabs
    }

    pub fn remove_pane(&mut self, kind: &PanelKind) {
        let plugin_id = kind
            .dynamic_id()
            .and_then(|id| self.plugin_panel_owners.remove(id));
        if let Some(id) = self.tree.tiles.find_pane(kind) {
            self.tree.remove_recursively(id);
        }
        if let Some(plugin_id) = plugin_id {
            self.collapse_plugin_group(&plugin_id);
        }
    }

    pub fn discard_dynamic_panes(&mut self) {
        let dynamic_ids: Vec<TileId> = self
            .tree
            .tiles
            .iter()
            .filter_map(|(id, tile)| match tile {
                Tile::Pane(kind) if kind.dynamic_id().is_some() => Some(*id),
                _ => None,
            })
            .collect();
        for id in dynamic_ids {
            self.tree.remove_recursively(id);
        }
        self.plugin_panel_owners.clear();
        self.plugin_groups.clear();
    }

    fn collapse_plugin_group(&mut self, plugin_id: &str) {
        let Some(group_id) = self.plugin_groups.get(plugin_id).copied() else {
            return;
        };
        let children = match self.tree.tiles.get(group_id) {
            Some(Tile::Container(Container::Tabs(tabs))) => tabs.children.clone(),
            _ => {
                self.plugin_groups.remove(plugin_id);
                return;
            }
        };

        match children.as_slice() {
            [] => {
                self.tree.remove_recursively(group_id);
            }
            [only_child] => {
                if let Some(parent_id) = self.tree.tiles.parent_of(group_id) {
                    if let Some(Tile::Container(parent)) = self.tree.tiles.get_mut(parent_id) {
                        parent.remove_child(group_id);
                        parent.add_child(*only_child);
                        if let Container::Tabs(tabs) = parent {
                            tabs.set_active(*only_child);
                        }
                    }
                } else {
                    self.tree.root = Some(*only_child);
                }
                self.tree.tiles.remove(group_id);
            }
            _ => return,
        }
        self.plugin_groups.remove(plugin_id);
    }

    pub fn plugin_group_id(&self, tile_id: TileId) -> Option<&str> {
        self.plugin_groups
            .iter()
            .find_map(|(plugin_id, id)| (*id == tile_id).then_some(plugin_id.as_str()))
    }

    pub fn is_pane_visible(&self, kind: &PanelKind) -> bool {
        self.tree
            .tiles
            .find_pane(kind)
            .is_some_and(|id| self.tree.active_tiles().contains(&id))
    }
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
    /// 新版工作区布局。None 表示从旧版 `dock` 字段自动迁移。
    #[serde(default)]
    pub tiles: Option<TilesLayout>,
}

impl Default for PanelManager {
    fn default() -> Self {
        Self {
            active_tab: PanelKind::Devices,
            inspector_visible: false,
            dock: DockLayout::default(),
            tiles: None,
        }
    }
}

impl PanelManager {
    /// 确保当前管理器拥有 tiles 布局；旧版配置会在首次调用时自动转换。
    pub fn ensure_tiles_layout(&mut self) -> &mut TilesLayout {
        if self.tiles.is_none() {
            self.dock.normalize_tool_layout();
            self.tiles = Some(TilesLayout::from_legacy(&self.dock));
        }
        self.tiles.as_mut().expect("tiles layout was initialized")
    }

    pub fn reset_tiles_layout(&mut self) {
        self.dock = DockLayout::default();
        self.tiles = Some(TilesLayout::from_legacy(&self.dock));
        self.active_tab = PanelKind::Devices;
    }

    pub fn bottom_visible(&mut self) -> bool {
        self.ensure_tiles_layout().bottom_visible()
    }

    pub fn set_bottom_visible(&mut self, visible: bool) {
        self.ensure_tiles_layout().set_bottom_visible(visible);
    }

    pub fn right_visible(&mut self) -> bool {
        self.ensure_tiles_layout().right_visible()
    }

    pub fn set_right_visible(&mut self, visible: bool) {
        self.ensure_tiles_layout().set_right_visible(visible);
    }

    pub fn plugin_group_id(&self, tile_id: TileId) -> Option<&str> {
        self.tiles.as_ref()?.plugin_group_id(tile_id)
    }

    /// 从 dock 的所有 stack 中派生 tabs 列表（唯一真相来源）
    pub fn tabs(&self) -> Vec<PanelKind> {
        if let Some(tiles) = self.tiles.as_ref() {
            return tiles
                .tree
                .tiles
                .iter()
                .filter_map(|(_, tile)| match tile {
                    Tile::Pane(kind) => Some(kind.clone()),
                    Tile::Container(_) => None,
                })
                .collect();
        }
        self.dock.all_tabs()
    }

    /// 切换 Center 标签栏的激活面板（左侧栏点击）。
    pub fn select_center_panel(&mut self, kind: PanelKind) {
        self.active_tab = kind.clone();
        self.ensure_tiles_layout().select_pane(&kind);
    }

    pub fn open_tab(&mut self, kind: PanelKind) {
        self.active_tab = kind.clone();
        self.ensure_tiles_layout().add_to_main_tabs(kind, true);
    }

    pub fn open_plugin_tab(&mut self, kind: PanelKind, plugin_id: &str) {
        self.active_tab = kind.clone();
        self.ensure_tiles_layout()
            .add_to_plugin_tabs(kind, plugin_id);
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
        if let Some(tiles) = self.tiles.as_ref() {
            return tiles.is_pane_visible(kind);
        }
        self.dock.center.active_or_first().as_ref() == Some(kind)
            || (self.dock.bottom_visible
                && self.dock.bottom.active_or_first().as_ref() == Some(kind))
            || (self.dock.right_visible && self.dock.right.active_or_first().as_ref() == Some(kind))
    }

    pub fn close_tab(&mut self, kind: PanelKind) {
        if self.tiles.is_some() {
            let fallback = self
                .tiles
                .as_ref()
                .and_then(|layout| {
                    layout.tree.tiles.iter().find_map(|(_, tile)| match tile {
                        Tile::Pane(candidate)
                            if candidate != &kind && candidate.dynamic_id().is_some() =>
                        {
                            Some(candidate.clone())
                        }
                        _ => None,
                    })
                })
                .unwrap_or(PanelKind::Devices);
            self.ensure_tiles_layout().remove_pane(&kind);
            if self.active_tab == kind {
                self.active_tab = fallback;
            }
            return;
        }
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
        if let Some(tiles) = self.tiles.as_mut() {
            tiles.discard_dynamic_panes();
        }
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

    #[test]
    fn legacy_dock_migrates_to_tiles_and_preserves_visibility() {
        let mut manager = PanelManager::default();
        manager.dock.bottom_visible = false;
        manager.dock.right_visible = true;
        manager.dock.right.open(PanelKind::Logs);

        let layout = manager.ensure_tiles_layout();

        assert!(!layout.bottom_visible());
        assert!(layout.right_visible());
        assert!(layout.tree.tiles.find_pane(&PanelKind::Devices).is_some());
        assert!(layout.tree.tiles.find_pane(&PanelKind::Logs).is_some());
    }

    #[test]
    fn dynamic_pane_is_added_to_and_removed_from_tiles() {
        let mut manager = PanelManager::default();
        let dynamic = PanelKind::Dynamic("runtime".to_owned());

        manager.open_tab(dynamic.clone());
        assert!(
            manager
                .ensure_tiles_layout()
                .tree
                .tiles
                .find_pane(&dynamic)
                .is_some()
        );

        manager.close_tab(dynamic.clone());
        assert!(
            manager
                .ensure_tiles_layout()
                .tree
                .tiles
                .find_pane(&dynamic)
                .is_none()
        );
    }

    #[test]
    fn opening_dynamic_pane_recovers_when_main_tabs_was_pruned() {
        let mut manager = PanelManager::default();
        let removed_main_tabs = manager.ensure_tiles_layout().main_tabs;
        manager
            .ensure_tiles_layout()
            .tree
            .remove_recursively(removed_main_tabs);

        let dynamic = PanelKind::Dynamic("runtime".to_owned());
        manager.open_tab(dynamic.clone());
        assert_eq!(manager.active_tab, dynamic);

        let layout = manager.ensure_tiles_layout();
        let pane_id = layout
            .tree
            .tiles
            .find_pane(&dynamic)
            .expect("dynamic pane should be inserted");
        assert!(layout.tree.active_tiles().contains(&pane_id));
    }

    #[test]
    fn plugin_dynamic_panes_group_and_collapse_back_to_one_pane() {
        let mut manager = PanelManager::default();
        let first = PanelKind::Dynamic("demo.chart".to_owned());
        let second = PanelKind::Dynamic("demo.form".to_owned());
        let third = PanelKind::Dynamic("demo.gauge".to_owned());

        manager.open_plugin_tab(first.clone(), "demo");
        manager.open_plugin_tab(second.clone(), "demo");
        manager.open_plugin_tab(third.clone(), "demo");

        {
            let layout = manager.ensure_tiles_layout();
            let group_id = *layout
                .plugin_groups
                .get("demo")
                .expect("second plugin pane should create a group");
            let first_id = layout.tree.tiles.find_pane(&first).expect("first pane");
            let second_id = layout.tree.tiles.find_pane(&second).expect("second pane");
            let Some(Tile::Container(Container::Tabs(tabs))) = layout.tree.tiles.get(group_id)
            else {
                panic!("plugin group should be a tab container");
            };
            assert_eq!(tabs.children.len(), 3);
            assert_eq!(tabs.children[..2], [first_id, second_id]);
        }

        manager.close_tab(third);
        manager.close_tab(second);

        let layout = manager.ensure_tiles_layout();
        assert!(!layout.plugin_groups.contains_key("demo"));
        assert!(layout.tree.tiles.find_pane(&first).is_some());
    }

    #[test]
    fn tiles_layout_round_trips_through_workspace_json() {
        let mut manager = PanelManager::default();
        manager.ensure_tiles_layout().set_right_visible(true);

        let json = serde_json::to_string(&manager).expect("serialize tiles layout");
        let restored: PanelManager = serde_json::from_str(&json).expect("deserialize tiles layout");

        assert!(restored.tiles.is_some());
        assert!(restored.tiles.expect("tiles layout").right_visible());
    }
}

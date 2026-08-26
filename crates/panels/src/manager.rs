use egui_tiles::{Container, Linear, LinearDir, Tile, TileId, Tiles, Tree};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

/// 面板标识（布局树中的 pane 类型）。
///
/// 所有面板——内置与插件动态面板——统一为字符串 ID：
///
/// - 内置面板：`"devices"`、`"terminal"` 等（见 `PANEL_*` 常量）；
/// - 插件动态面板：`"dynamic:<id>"` 前缀，与内置 id 空间隔离。
///
/// # 配置兼容
///
/// 旧版 `PanelKind` 枚举序列化为 `"devices"` 或 `{"dynamic": "abc"}`，
/// 本类型反序列化时同时接受这两种旧格式，`workspace.json` 无需迁移。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PanelId(String);

/// 内置面板 ID。与旧版 `PanelKind` 的 snake_case 序列化格式完全一致，
/// 保证旧配置可直接加载。
pub const PANEL_DEVICES: &str = "devices";
pub const PANEL_REPLAY: &str = "replay";
pub const PANEL_PLUGINS: &str = "plugins";
pub const PANEL_SETTINGS: &str = "settings";
pub const PANEL_TERMINAL: &str = "terminal";
pub const PANEL_SENDER: &str = "sender";
pub const PANEL_LOGS: &str = "logs";
pub const PANEL_CHART: &str = "chart";

/// 内置面板 ID 全集（顺序即默认布局顺序）。
pub const PANEL_BUILTIN: [&str; 8] = [
    PANEL_DEVICES,
    PANEL_REPLAY,
    PANEL_PLUGINS,
    PANEL_SETTINGS,
    PANEL_TERMINAL,
    PANEL_SENDER,
    PANEL_LOGS,
    PANEL_CHART,
];

/// 动态面板 ID 前缀，与内置面板 id 空间隔离。
const DYNAMIC_PREFIX: &str = "dynamic:";

impl PanelId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// 内置面板 ID（校验通过内置白名单）。
    pub fn builtin(id: &str) -> Self {
        debug_assert!(
            PANEL_BUILTIN.contains(&id),
            "unknown builtin panel id: {id}"
        );
        Self(id.to_owned())
    }

    /// 插件动态面板 ID：加 `dynamic:` 前缀与内置面板隔离。
    pub fn dynamic(id: &str) -> Self {
        Self(format!("{DYNAMIC_PREFIX}{id}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_dynamic(&self) -> bool {
        self.0.starts_with(DYNAMIC_PREFIX)
    }

    /// 动态面板的裸 id（去掉 `dynamic:` 前缀），供 DynamicPanels 查询。
    pub fn dynamic_suffix(&self) -> Option<&str> {
        self.0.strip_prefix(DYNAMIC_PREFIX)
    }

    /// 是否为内置面板。
    pub fn is_builtin(&self) -> bool {
        PANEL_BUILTIN.contains(&self.0.as_str())
    }
}

impl std::fmt::Display for PanelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for PanelId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PanelId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PanelIdVisitor;

        impl<'de> serde::de::Visitor<'de> for PanelIdVisitor {
            type Value = PanelId;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a panel id string or legacy {\"dynamic\": ...} object")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(PanelId(value.to_owned()))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                // 旧版 PanelKind::Dynamic(String) 序列化为 {"dynamic": "abc"}。
                // 迁移为 "dynamic:abc"，与运行时新生成格式一致。
                while let Some(key) = map.next_key::<String>()? {
                    if key == "dynamic" {
                        let id: String = map.next_value()?;
                        return Ok(PanelId::dynamic(&id));
                    }
                    let _: serde::de::IgnoredAny = map.next_value()?;
                }
                Err(serde::de::Error::custom(
                    "panel id object must have a \"dynamic\" key",
                ))
            }
        }

        deserializer.deserialize_any(PanelIdVisitor)
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
    pub tabs: Vec<PanelId>,
    pub active: Option<PanelId>,
}

impl DockStack {
    pub fn open(&mut self, kind: PanelId) {
        if !self.tabs.contains(&kind) {
            self.tabs.push(kind.clone());
        }
        self.active = Some(kind);
    }

    /// 添加标签但不切换焦点（后台创建面板时使用）
    pub fn add_inactive(&mut self, kind: PanelId) {
        if !self.tabs.contains(&kind) {
            self.tabs.push(kind.clone());
        }
        if self.active.is_none() {
            self.active = Some(kind);
        }
    }

    pub fn close(&mut self, kind: &PanelId) {
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

    pub fn remove(&mut self, kind: &PanelId) -> bool {
        let old_len = self.tabs.len();
        self.close(kind);
        old_len != self.tabs.len()
    }

    pub fn contains(&self, kind: &PanelId) -> bool {
        self.tabs.contains(kind)
    }

    pub fn active_or_first(&self) -> Option<PanelId> {
        self.active
            .clone()
            .filter(|kind| self.tabs.contains(kind))
            .or_else(|| self.tabs.first().cloned())
    }

    pub(crate) fn discard_dynamic_tabs(&mut self) {
        self.tabs.retain(|kind| !kind.is_dynamic());
        if self.active.as_ref().is_some_and(PanelId::is_dynamic) {
            self.active = self.tabs.last().cloned();
        }
    }

    pub fn reorder(&mut self, kind: &PanelId, mut insert_index: usize) -> bool {
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
    pub tree: Tree<PanelId>,
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
    fn pane_ids(tiles: &mut Tiles<PanelId>, panes: &[PanelId]) -> Vec<TileId> {
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
            vec![PanelId::builtin(PANEL_DEVICES)]
        } else {
            dock.center.tabs.clone()
        };
        let bottom = if dock.bottom.tabs.is_empty() {
            vec![
                PanelId::builtin(PANEL_TERMINAL),
                PanelId::builtin(PANEL_LOGS),
                PanelId::builtin(PANEL_SENDER),
            ]
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

    /// 当前工作区的默认 Dock 布局。
    ///
    /// 该布局来自用户当前工作区：主标签区与发送器上下排列，接收区和日志
    /// 放在右侧。这里只固化面板树，不包含串口、主题、历史记录等个人配置。
    /// Native/Web 都从这里开始，再由各自的 capability 过滤不可用面板。
    pub fn current_default() -> Self {
        let mut tiles = Tiles::default();

        let center_panes = TilesLayout::pane_ids(
            &mut tiles,
            &[
                PanelId::builtin(PANEL_SETTINGS),
                PanelId::builtin(PANEL_DEVICES),
                PanelId::builtin(PANEL_REPLAY),
                PanelId::builtin(PANEL_PLUGINS),
            ],
        );
        let sender_pane = Self::pane_ids(&mut tiles, &[PanelId::builtin(PANEL_SENDER)]);
        let side_panes = Self::pane_ids(
            &mut tiles,
            &[
                PanelId::builtin(PANEL_LOGS),
                PanelId::builtin(PANEL_TERMINAL),
            ],
        );

        let main_tabs = tiles.insert_tab_tile(center_panes);
        let bottom_tabs = tiles.insert_tab_tile(sender_pane);
        let right_tabs = tiles.insert_tab_tile(side_panes);
        let settings_pane = tiles
            .find_pane(&PanelId::builtin(PANEL_SETTINGS))
            .expect("default settings pane");
        let terminal_pane = tiles
            .find_pane(&PanelId::builtin(PANEL_TERMINAL))
            .expect("default terminal pane");

        if let Some(Tile::Container(Container::Tabs(tabs))) = tiles.get_mut(main_tabs) {
            tabs.set_active(settings_pane);
        }
        if let Some(Tile::Container(Container::Tabs(tabs))) = tiles.get_mut(right_tabs) {
            tabs.set_active(terminal_pane);
        }

        let main_column = tiles.insert_vertical_tile(vec![main_tabs, bottom_tabs]);
        let mut root_linear = Linear::new(LinearDir::Horizontal, vec![main_column, right_tabs]);
        // Keep the release layout's persisted proportions for both Native and
        // Web. Existing user layouts remain untouched; this only controls the
        // default used by reset/default-workspace creation.
        root_linear.shares.set_share(main_column, 0.45367718);
        root_linear.shares.set_share(right_tabs, 0.66292167);
        let root = tiles.insert_container(root_linear);
        let tree = Tree::new("hardware-workbench-layout", root, tiles);

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

    pub fn select_pane(&mut self, kind: &PanelId) -> bool {
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

    pub fn add_to_main_tabs(&mut self, kind: PanelId, activate: bool) {
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
    pub fn add_to_plugin_tabs(&mut self, kind: PanelId, plugin_id: &str) {
        let Some(panel_id) = kind.dynamic_suffix().map(str::to_owned) else {
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
            .and_then(|(id, _)| self.tree.tiles.find_pane(&PanelId::dynamic(id)));

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

    pub fn remove_pane(&mut self, kind: &PanelId) {
        let plugin_id = kind
            .dynamic_suffix()
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
                Tile::Pane(kind) if kind.is_dynamic() => Some(*id),
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

    /// 拖拽可以改变任意容器的父子关系。插件分组只是“由运行时自动创建”的
    /// 辅助元数据，不能在用户把其中的面板拆走后继续给一个普通标签组冠上插件名。
    ///
    /// 只保留仍然直接包含两个及以上、且全部归属于同一插件的动态面板的分组。
    /// 布局本身完全由用户的拖拽结果决定，不会在这里重新拼接或移动任何面板。
    pub fn reconcile_plugin_groups(&mut self) -> bool {
        let owners_before = self.plugin_panel_owners.len();
        self.plugin_panel_owners.retain(|panel_id, _| {
            self.tree
                .tiles
                .find_pane(&PanelId::dynamic(panel_id))
                .is_some()
        });

        let groups_before = self.plugin_groups.len();
        self.plugin_groups.retain(|plugin_id, group_id| {
            let Some(Tile::Container(Container::Tabs(tabs))) = self.tree.tiles.get(*group_id)
            else {
                return false;
            };

            tabs.children.len() >= 2
                && tabs.children.iter().all(|child_id| {
                    matches!(
                        self.tree.tiles.get(*child_id),
                        Some(Tile::Pane(panel_id))
                            if panel_id.is_dynamic()
                                && self
                                    .plugin_panel_owners
                                    .get(panel_id.dynamic_suffix().unwrap_or(""))
                                    == Some(plugin_id)
                    )
                })
        });

        owners_before != self.plugin_panel_owners.len() || groups_before != self.plugin_groups.len()
    }

    pub fn is_pane_visible(&self, kind: &PanelId) -> bool {
        self.tree
            .tiles
            .find_pane(kind)
            .is_some_and(|id| self.tree.active_tiles().contains(&id))
    }
}

impl Default for DockLayout {
    fn default() -> Self {
        let mut center = DockStack::default();
        center.open(PanelId::builtin(PANEL_DEVICES));
        center.open(PanelId::builtin(PANEL_REPLAY));
        center.open(PanelId::builtin(PANEL_PLUGINS));
        center.open(PanelId::builtin(PANEL_SETTINGS));

        let mut bottom = DockStack::default();
        bottom.open(PanelId::builtin(PANEL_TERMINAL));
        bottom.open(PanelId::builtin(PANEL_LOGS));
        bottom.open(PanelId::builtin(PANEL_SENDER));
        bottom.active = Some(PanelId::builtin(PANEL_TERMINAL));

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

    pub fn move_panel(&mut self, kind: PanelId, to: DockArea) {
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
    pub fn insert_panel_at(&mut self, kind: PanelId, to: DockArea, index: usize) {
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

    pub fn all_tabs(&self) -> Vec<PanelId> {
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
        for kind in [
            PanelId::builtin(PANEL_TERMINAL),
            PanelId::builtin(PANEL_LOGS),
        ] {
            if self.center.remove(&kind) {
                if !self.bottom.contains(&kind) && !self.right.contains(&kind) {
                    self.bottom.open(kind);
                }
                self.bottom_visible = true;
            }
        }

        // Sender 不允许在 Center stack 中存在
        self.center.remove(&PanelId::builtin(PANEL_SENDER));

        // Sender 如果没有在任何区域，默认放到底部
        if !self.bottom.contains(&PanelId::builtin(PANEL_SENDER))
            && !self.right.contains(&PanelId::builtin(PANEL_SENDER))
        {
            self.bottom.open(PanelId::builtin(PANEL_SENDER));
        }

        // 确保功能面板（回放、插件、设置）至少在一个停靠区中存在。
        // 避免旧版配置文件或异常重置后用户看不到这些核心功能入口。
        for kind in [
            PanelId::builtin(PANEL_REPLAY),
            PanelId::builtin(PANEL_PLUGINS),
            PanelId::builtin(PANEL_SETTINGS),
        ] {
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
    pub active_tab: PanelId,
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
            active_tab: PanelId::builtin(PANEL_DEVICES),
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
        // 布局重置不等于关闭插件：保留当前运行期动态面板及其插件归属，
        // 在默认布局中重新建立它们，避免用户恢复布局后找不到插件窗口。
        let dynamic_panels: Vec<(PanelId, Option<String>)> = self
            .tiles
            .as_ref()
            .map(|layout| {
                layout
                    .tree
                    .tiles
                    .iter()
                    .filter_map(|(_, tile)| match tile {
                        Tile::Pane(kind) if kind.is_dynamic() => Some((
                            kind.clone(),
                            layout
                                .plugin_panel_owners
                                .get(kind.dynamic_suffix().unwrap_or(""))
                                .cloned(),
                        )),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        self.dock = DockLayout::default();
        self.tiles = Some(TilesLayout::current_default());
        if let Some(layout) = self.tiles.as_mut() {
            for (kind, plugin_id) in dynamic_panels {
                if let Some(plugin_id) = plugin_id {
                    layout.add_to_plugin_tabs(kind, &plugin_id);
                } else {
                    layout.add_to_main_tabs(kind, false);
                }
            }
            layout.select_pane(&PanelId::builtin(PANEL_DEVICES));
        }
        self.active_tab = PanelId::builtin(PANEL_TERMINAL);
    }

    /// 创建没有用户配置文件时使用的工作区默认值。
    pub fn default_workspace() -> Self {
        Self {
            active_tab: PanelId::builtin(PANEL_TERMINAL),
            tiles: Some(TilesLayout::current_default()),
            ..Self::default()
        }
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
    pub fn tabs(&self) -> Vec<PanelId> {
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
    pub fn select_center_panel(&mut self, kind: PanelId) {
        self.active_tab = kind.clone();
        self.ensure_tiles_layout().select_pane(&kind);
    }

    pub fn open_tab(&mut self, kind: PanelId) {
        self.active_tab = kind.clone();
        self.ensure_tiles_layout().add_to_main_tabs(kind, true);
    }

    pub fn open_plugin_tab(&mut self, kind: PanelId, plugin_id: &str) {
        self.active_tab = kind.clone();
        self.ensure_tiles_layout()
            .add_to_plugin_tabs(kind, plugin_id);
    }

    /// 同步 active_tab 到 Center 当前激活面板（Center 渲染后调用）。
    pub fn sync_active_tab_from_center(&mut self) {
        if let Some(kind) = self.dock.center.active_or_first() {
            self.active_tab = kind;
        } else {
            self.active_tab = self
                .dock
                .all_tabs()
                .first()
                .cloned()
                .unwrap_or_else(|| PanelId::builtin(PANEL_DEVICES));
        }
    }

    pub fn is_panel_visible(&self, kind: &PanelId) -> bool {
        if let Some(tiles) = self.tiles.as_ref() {
            return tiles.is_pane_visible(kind);
        }
        self.dock.center.active_or_first().as_ref() == Some(kind)
            || (self.dock.bottom_visible
                && self.dock.bottom.active_or_first().as_ref() == Some(kind))
            || (self.dock.right_visible && self.dock.right.active_or_first().as_ref() == Some(kind))
    }

    pub fn close_tab(&mut self, kind: PanelId) {
        if self.tiles.is_some() {
            let fallback = self
                .tiles
                .as_ref()
                .and_then(|layout| {
                    layout.tree.tiles.iter().find_map(|(_, tile)| match tile {
                        Tile::Pane(candidate) if candidate != &kind && candidate.is_dynamic() => {
                            Some(candidate.clone())
                        }
                        _ => None,
                    })
                })
                .unwrap_or_else(|| PanelId::builtin(PANEL_DEVICES));
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
                .unwrap_or_else(|| PanelId::builtin(PANEL_DEVICES));
        }
        self.dock.center.remove(&kind);
        self.dock.bottom.remove(&kind);
        self.dock.right.remove(&kind);
    }

    pub fn active_dynamic_id(&self) -> Option<&str> {
        self.active_tab.dynamic_suffix()
    }

    pub fn discard_dynamic_tabs(&mut self) {
        if let Some(tiles) = self.tiles.as_mut() {
            tiles.discard_dynamic_panes();
        }
        if self.active_tab.is_dynamic() {
            self.active_tab = self
                .dock
                .center
                .tabs
                .iter()
                .find(|k| !k.is_dynamic())
                .cloned()
                .unwrap_or_else(|| PanelId::builtin(PANEL_DEVICES));
        }
        self.dock.discard_dynamic_tabs();
    }

    /// 在 dock.move_panel() 调用后同步 active_tab
    pub fn sync_tabs_from_dock(&mut self) {
        let all_tabs = self.dock.all_tabs();
        if !all_tabs.contains(&self.active_tab) {
            self.active_tab = all_tabs
                .first()
                .cloned()
                .unwrap_or_else(|| PanelId::builtin(PANEL_DEVICES));
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
        manager.dock.center.open(PanelId::builtin(PANEL_REPLAY));

        manager.select_center_panel(PanelId::builtin(PANEL_REPLAY));

        assert_eq!(manager.active_tab, PanelId::builtin(PANEL_REPLAY));
        assert_eq!(
            manager.dock.center.active,
            Some(PanelId::builtin(PANEL_REPLAY))
        );
    }

    #[test]
    fn closing_active_dynamic_falls_back_to_previous_dynamic() {
        let mut manager = PanelManager::default();
        let first = PanelId::dynamic("a");
        let second = PanelId::dynamic("b");
        manager.open_tab(first.clone());
        manager.open_tab(second.clone());

        manager.close_tab(second);

        assert_eq!(manager.active_tab, first);
    }

    #[test]
    fn discard_dynamic_tabs_removes_transient_state() {
        let mut manager = PanelManager::default();
        manager.open_tab(PanelId::dynamic("runtime"));

        manager.discard_dynamic_tabs();

        assert!(!manager.tabs().iter().any(|k| k.is_dynamic()));
        assert!(manager.active_dynamic_id().is_none());
    }

    #[test]
    fn sync_tabs_from_dock_uses_center_active_panel() {
        let mut manager = PanelManager {
            active_tab: PanelId::builtin(PANEL_DEVICES),
            ..Default::default()
        };
        manager.dock.center.open(PanelId::builtin(PANEL_SETTINGS));

        manager.sync_tabs_from_dock();

        assert_eq!(manager.active_tab, PanelId::builtin(PANEL_SETTINGS));
    }

    #[test]
    fn is_panel_visible_checks_active_dock_stacks() {
        let mut manager = PanelManager::default();
        manager.dock.center.open(PanelId::builtin(PANEL_SETTINGS));
        manager.dock.bottom_visible = true;
        manager.dock.bottom.open(PanelId::builtin(PANEL_LOGS));
        manager.dock.right_visible = false;
        manager.dock.right.open(PanelId::builtin(PANEL_TERMINAL));

        assert!(manager.is_panel_visible(&PanelId::builtin(PANEL_SETTINGS)));
        assert!(manager.is_panel_visible(&PanelId::builtin(PANEL_LOGS)));
        assert!(!manager.is_panel_visible(&PanelId::builtin(PANEL_TERMINAL)));
    }

    #[test]
    fn legacy_dock_migrates_to_tiles_and_preserves_visibility() {
        let mut manager = PanelManager::default();
        manager.dock.bottom_visible = false;
        manager.dock.right_visible = true;
        manager.dock.right.open(PanelId::builtin(PANEL_LOGS));

        let layout = manager.ensure_tiles_layout();

        assert!(!layout.bottom_visible());
        assert!(layout.right_visible());
        assert!(
            layout
                .tree
                .tiles
                .find_pane(&PanelId::builtin(PANEL_DEVICES))
                .is_some()
        );
        assert!(
            layout
                .tree
                .tiles
                .find_pane(&PanelId::builtin(PANEL_LOGS))
                .is_some()
        );
    }

    #[test]
    fn dynamic_pane_is_added_to_and_removed_from_tiles() {
        let mut manager = PanelManager::default();
        let dynamic = PanelId::dynamic("runtime");

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

        let dynamic = PanelId::dynamic("runtime");
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
        let first = PanelId::dynamic("demo.chart");
        let second = PanelId::dynamic("demo.form");
        let third = PanelId::dynamic("demo.gauge");

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
    fn dragging_a_plugin_panel_out_clears_the_automatic_group_label() {
        let mut manager = PanelManager::default();
        let first = PanelId::dynamic("demo.chart");
        let second = PanelId::dynamic("demo.form");
        manager.open_plugin_tab(first.clone(), "demo");
        manager.open_plugin_tab(second.clone(), "demo");

        let layout = manager.ensure_tiles_layout();
        let group_id = *layout.plugin_groups.get("demo").expect("plugin group");
        let second_id = layout.tree.tiles.find_pane(&second).expect("second pane");
        layout.tree.remove_recursively(second_id);

        assert!(layout.reconcile_plugin_groups());
        assert!(!layout.plugin_groups.contains_key("demo"));
        assert_eq!(layout.plugin_group_id(group_id), None);
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

    #[test]
    fn resetting_layout_preserves_running_plugin_panels() {
        let mut manager = PanelManager::default();
        let first = PanelId::dynamic("demo.chart");
        let second = PanelId::dynamic("demo.form");
        manager.open_plugin_tab(first.clone(), "demo");
        manager.open_plugin_tab(second.clone(), "demo");

        manager.reset_tiles_layout();

        let layout = manager.ensure_tiles_layout();
        assert!(layout.tree.tiles.find_pane(&first).is_some());
        assert!(layout.tree.tiles.find_pane(&second).is_some());
        assert_eq!(
            layout
                .plugin_panel_owners
                .get("demo.chart")
                .map(String::as_str),
            Some("demo")
        );
        assert!(layout.plugin_groups.contains_key("demo"));
        assert_eq!(manager.active_tab, PanelId::builtin(PANEL_TERMINAL));
    }

    #[test]
    fn panel_id_accepts_legacy_dynamic_object() {
        // 旧版 PanelKind::Dynamic("abc") 序列化为 {"dynamic":"abc"}
        let restored: PanelId = serde_json::from_str(r#"{"dynamic":"abc"}"#).unwrap();
        assert_eq!(restored, PanelId::dynamic("abc"));
        assert!(restored.is_dynamic());
        assert_eq!(restored.dynamic_suffix(), Some("abc"));
    }

    #[test]
    fn panel_id_accepts_plain_string() {
        let restored: PanelId = serde_json::from_str(r#""devices""#).unwrap();
        assert_eq!(restored, PanelId::builtin(PANEL_DEVICES));
        assert!(restored.is_builtin());
    }

    #[test]
    fn panel_id_round_trips_as_plain_string() {
        let id = PanelId::dynamic("abc");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, r#""dynamic:abc""#);
        let restored: PanelId = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, id);
    }
}

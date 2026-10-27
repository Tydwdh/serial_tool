//! Shared egui/egui_tiles shell used by Native and Web.

use crate::panel_registry::{BuiltinPanel, PanelKind, PanelRegistry};
use eframe::egui;
use egui_tiles::{
    Behavior, Container, ContainerKind, DragPreview, EditAction, ResizeState,
    SimplificationOptions, TabState, Tile, TileId, Tiles, UiResponse,
};
use tool_panels::{
    ChartPanel, DynamicPanels, LogPanel, PanelId, PanelManager, TerminalPanel, theme,
};

pub(crate) trait DockHost {
    /// Native measures with `Instant`; web measures with a JS timestamp.
    type RenderStart: Copy;

    fn panels(&mut self) -> &mut PanelManager;
    fn panels_ref(&self) -> &PanelManager;
    fn panel_registry(&self) -> &PanelRegistry;
    fn shared_panels(&mut self) -> SharedPanels<'_>;
    /// Render the panels whose bodies genuinely differ per platform.
    fn render_platform_panel(&mut self, ui: &mut egui::Ui, kind: BuiltinPanel);
    /// Push platform-specific state into the shared Terminal panel.
    fn prepare_terminal_panel(&mut self) {}
    fn panel_render_start(&self) -> Self::RenderStart;
    fn note_panel_render(&mut self, _kind: BuiltinPanel, _started: Self::RenderStart) {}
    fn note_dynamic_panel_render(&mut self, _started: Self::RenderStart) {}
    fn mark_layout_dirty(&mut self);
}

/// The shared panel bodies, handed to the shell through one accessor.
pub(crate) struct SharedPanels<'a> {
    pub terminal: &'a mut TerminalPanel,
    pub chart: &'a mut ChartPanel,
    pub log: &'a mut LogPanel,
    pub dynamic: &'a mut DynamicPanels,
}

impl SharedPanels<'_> {
    fn render_dynamic(&mut self, ui: &mut egui::Ui, suffix: &str) {
        let _ = egui::ScrollArea::vertical().show(ui, |ui| self.dynamic.ui_body(ui, suffix));
    }
}

/// The single built-in panel dispatch. Shared bodies are rendered here; the
/// platform leaves are delegated to [`DockHost::render_platform_panel`].
pub(crate) fn render_panel<H: DockHost>(host: &mut H, ui: &mut egui::Ui, id: &PanelId) {
    host.panels().active_tab = id.clone();
    let (available, title, kind) = {
        let registry = host.panel_registry();
        (
            registry.is_available(id),
            registry.title(id),
            registry.kind_for(id),
        )
    };
    if !available {
        ui.vertical_centered(|ui| {
            ui.add_space(32.0);
            ui.heading(title);
            ui.label("当前运行环境没有提供这个面板所需的平台能力");
            ui.small("这不是布局占位；请检查浏览器权限、设备能力或插件运行状态。");
        });
        return;
    }
    match kind {
        Some(PanelKind::Builtin(kind)) => match kind {
            BuiltinPanel::Terminal => {
                host.prepare_terminal_panel();
                let started = host.panel_render_start();
                host.shared_panels().terminal.ui(ui);
                host.note_panel_render(kind, started);
            }
            BuiltinPanel::Chart => {
                let started = host.panel_render_start();
                host.shared_panels().chart.ui(ui);
                host.note_panel_render(kind, started);
            }
            BuiltinPanel::Logs => {
                let started = host.panel_render_start();
                host.shared_panels().log.ui(ui);
                host.note_panel_render(kind, started);
            }
            _ => host.render_platform_panel(ui, kind),
        },
        Some(PanelKind::Dynamic { suffix }) => {
            if host.shared_panels().dynamic.contains(&suffix) {
                let started = host.panel_render_start();
                host.shared_panels().render_dynamic(ui, &suffix);
                host.note_dynamic_panel_render(started);
            } else {
                ui.colored_label(theme::red(), format!("动态面板不存在：{suffix}"));
            }
        }
        None => {
            ui.colored_label(theme::red(), format!("面板不存在：{id}"));
        }
    }
}

/// Shared application chrome. Native and Web provide only the contents of the
/// two bars; their frame, sizing, ids and central margins stay identical.
pub(crate) trait AppShellHost: DockHost {
    fn render_top_bar(&mut self, ui: &mut egui::Ui);
    fn render_status_bar(&mut self, ui: &mut egui::Ui);

    fn after_dock(&mut self, _ui: &egui::Ui) {}
}

pub(crate) fn show_shell<H: AppShellHost>(host: &mut H, ui: &mut egui::Ui) {
    egui::Panel::top("top-bar")
        .frame(
            egui::Frame::new()
                .fill(theme::bg_secondary())
                .stroke(egui::Stroke::new(1.0, theme::border()))
                .inner_margin(egui::Margin::symmetric(14, 8)),
        )
        .show(ui, |ui| host.render_top_bar(ui));

    // Fixed status bar: it must not participate in the resizable bottom dock.
    egui::Panel::bottom("status-bar")
        .resizable(false)
        .exact_size(34.0)
        .show_separator_line(true)
        .frame(
            egui::Frame::new()
                .fill(theme::bg_secondary())
                .inner_margin(egui::Margin::symmetric(14, 4)),
        )
        .show(ui, |ui| host.render_status_bar(ui));

    egui::CentralPanel::default()
        .frame(egui::Frame::default().fill(theme::bg_deep()))
        .show(ui, |ui| {
            egui::Frame::default()
                .fill(theme::bg_deep())
                .inner_margin(egui::Margin::symmetric(14, 8))
                .show(ui, |ui| show_dock(host, ui));
        });
    host.after_dock(ui);
}

pub(crate) fn show_dock<H: DockHost>(host: &mut H, ui: &mut egui::Ui) {
    let original_layout = host.panels().ensure_tiles_layout().clone();
    let mut layout = original_layout.clone();
    {
        let mut behavior = SharedTiles { host };
        layout.tree.ui(&mut behavior, ui);
    }
    // 记住底部/右侧区域当前容纳的面板：区域被拖空后容器会被剪掉，这份记录是
    // 「显示底部/右侧面板」重建该区域的依据。
    layout.remember_region_panes();
    if layout.reconcile_plugin_groups() {
        host.mark_layout_dirty();
    }
    // 渲染期间若另有逻辑整体换掉了 `tiles`，就不能用本次的旧副本把它覆盖回去：
    // 只有 `tiles` 仍是进来时那一份时才写回。
    if host.panels().tiles.as_ref() == Some(&original_layout) {
        host.panels().tiles = Some(layout);
    }
}

struct SharedTiles<'a, H> {
    host: &'a mut H,
}

/// 1px 分隔线的统一构造：颜色全透明时退化为 `Stroke::NONE`。
/// tab 外框与 tab 栏下划线共用这条判定。
fn hairline(color: egui::Color32) -> egui::Stroke {
    if color.a() == 0 {
        egui::Stroke::NONE
    } else {
        egui::Stroke::new(1.0, color)
    }
}

impl<H: DockHost> SharedTiles<'_, H> {
    fn title(&self, id: &PanelId) -> String {
        self.host.panel_registry().title(id)
    }

    fn icon(&self, id: &PanelId) -> egui_material_icons::MaterialIcon {
        self.host.panel_registry().icon(id)
    }
}

impl<H: DockHost> Behavior<PanelId> for SharedTiles<'_, H> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile_id: TileId, pane: &mut PanelId) -> UiResponse {
        render_panel(self.host, ui, pane);
        UiResponse::None
    }

    fn tab_hover_cursor_icon(&self) -> egui::CursorIcon {
        egui::CursorIcon::Default
    }

    fn tab_title_for_pane(&mut self, pane: &PanelId) -> egui::WidgetText {
        format!("{} {}", self.icon(pane).codepoint, self.title(pane)).into()
    }

    fn tab_title_for_tile(&mut self, tiles: &Tiles<PanelId>, tile_id: TileId) -> egui::WidgetText {
        if let Some(plugin_id) = self.host.panels().plugin_group_id(tile_id) {
            return format!(
                "{} {plugin_id}",
                egui_material_icons::icons::ICON_CABLE.codepoint
            )
            .into();
        }
        match tiles.get(tile_id) {
            Some(Tile::Pane(pane)) => self.tab_title_for_pane(pane),
            Some(Tile::Container(Container::Tabs(tabs))) => {
                // 只装了一个面板的标签组仍显示那个面板的标题；多标签才显示组名。
                let single_child = tabs.children.first().filter(|_| tabs.children.len() == 1);
                match single_child.and_then(|child_id| tiles.get(*child_id)) {
                    Some(Tile::Pane(pane)) => self.tab_title_for_pane(pane),
                    _ => format!("标签组（{}）", tabs.children.len()).into(),
                }
            }
            Some(Tile::Container(container)) => match container.kind() {
                // 容器被当成标签显示时（拖拽产生的嵌套容器），给出可读的中文名，
                // 而不是 `ContainerKind` 的 Debug 输出（"Horizontal"/"Vertical"）。
                ContainerKind::Horizontal => "左右拆分".into(),
                ContainerKind::Vertical => "上下拆分".into(),
                ContainerKind::Tabs => "标签组".into(),
                ContainerKind::Grid => "网格".into(),
            },
            None => "MISSING TILE".into(),
        }
    }

    fn tab_bar_height(&self, _style: &egui::Style) -> f32 {
        40.0
    }

    fn tab_title_spacing(&self, _visuals: &egui::Visuals) -> f32 {
        14.0
    }

    fn tab_bar_color(&self, _visuals: &egui::Visuals) -> egui::Color32 {
        theme::tab_bar_bg()
    }

    fn tab_bg_color(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<PanelId>,
        _tile_id: TileId,
        state: &TabState,
    ) -> egui::Color32 {
        if state.active {
            theme::tab_active_bg()
        } else {
            theme::tab_inactive_bg()
        }
    }

    fn tab_outline_stroke(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<PanelId>,
        _tile_id: TileId,
        state: &TabState,
    ) -> egui::Stroke {
        if state.active {
            hairline(theme::tab_active_outline())
        } else {
            egui::Stroke::NONE
        }
    }

    fn tab_bar_hline_stroke(&self, _visuals: &egui::Visuals) -> egui::Stroke {
        hairline(theme::tab_bar_outline())
    }

    fn tab_text_color(
        &self,
        _visuals: &egui::Visuals,
        _tiles: &Tiles<PanelId>,
        _tile_id: TileId,
        state: &TabState,
    ) -> egui::Color32 {
        if state.active {
            theme::tab_active_text()
        } else {
            theme::tab_inactive_text()
        }
    }

    fn retain_pane(&mut self, pane: &PanelId) -> bool {
        self.host.panel_registry().contains(pane)
    }

    fn on_edit(&mut self, _action: EditAction) {
        self.host.mark_layout_dirty();
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        5.0
    }

    fn resize_stroke(&self, _style: &egui::Style, state: ResizeState) -> egui::Stroke {
        match state {
            ResizeState::Idle => egui::Stroke::new(2.0, theme::separator_strong()),
            ResizeState::Hovering => egui::Stroke::new(3.0, theme::blue()),
            ResizeState::Dragging => egui::Stroke::new(3.0, theme::cyan()),
        }
    }

    fn paint_drag_preview(
        &self,
        _visuals: &egui::Visuals,
        painter: &egui::Painter,
        preview: DragPreview,
    ) {
        // 目标区域（有父容器时用父容器）的中心：既是拆分方向的参照点，也是徽标的位置。
        let badge_center = preview.parent_rect.unwrap_or(preview.target_rect).center();
        let (label, color) = match preview.insertion_kind {
            ContainerKind::Tabs => {
                let label = self
                    .host
                    .panels_ref()
                    .tiles
                    .as_ref()
                    .and_then(|layout| layout.tree.tiles.get(preview.target_tile_id))
                    .map(|target| match target {
                        Tile::Pane(pane) => format!("与「{}」组成标签组", self.title(pane)),
                        Tile::Container(Container::Tabs(tabs)) => {
                            format!("加入当前标签组（{} 个标签）", tabs.children.len())
                        }
                        Tile::Container(_) => "将目标区域合并为标签组".to_owned(),
                    })
                    .unwrap_or_else(|| "合并为标签".to_owned());
                (label, theme::purple())
            }
            ContainerKind::Horizontal => {
                let label = if preview.preview_rect.center().x <= badge_center.x {
                    "左侧拆分"
                } else {
                    "右侧拆分"
                };
                (label.to_owned(), theme::blue())
            }
            ContainerKind::Vertical => {
                let label = if preview.preview_rect.center().y <= badge_center.y {
                    "上方拆分"
                } else {
                    "下方拆分"
                };
                (label.to_owned(), theme::cyan())
            }
            ContainerKind::Grid => ("加入网格".to_owned(), theme::green()),
        };

        painter.rect(
            preview.preview_rect,
            4.0,
            color.linear_multiply(0.20),
            egui::Stroke::new(2.0, color),
            egui::StrokeKind::Inside,
        );
        let badge_width = (label.chars().count() as f32 * 16.0 + 32.0).clamp(132.0, 300.0);
        let badge_rect = egui::Rect::from_center_size(badge_center, egui::vec2(badge_width, 30.0));
        painter.rect(
            badge_rect,
            6.0,
            theme::bg_primary(),
            egui::Stroke::new(1.5, color),
            egui::StrokeKind::Inside,
        );
        painter.text(
            badge_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(16.0),
            theme::text_white(),
        );
    }

    fn min_size(&self) -> f32 {
        120.0
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions {
            all_panes_must_have_tabs: true,
            prune_empty_tabs: true,
            prune_empty_containers: true,
            prune_single_child_tabs: false,
            ..Default::default()
        }
    }
}

# 双击任意位置跳转 + 目标行高亮淡出

## 背景

接收区（Terminal）与日志区（Log）支持「搜索时双击结果 → 离开搜索进入上下文」的跳转。

当前两个限制：

1. **双击只能点文字**：双击判定用 `ctx_response.double_clicked()`（`terminal.rs:1603`、`log.rs:756`），而 `ctx_response` 只累加每行**文本列**的 `ui.interact` response。空白区（行尾 padding、行间、元数据列）走 `handle_input` 的全局指针判定，没有 response，因此双击空白不触发跳转。
2. **跳转后无高亮**：跳转只 `scroll_to_rect` 滚动到目标行（`terminal.rs:1617-1623`、`log.rs:764`），没有任何视觉标记。行多时用户看不清跳到了哪一行。

## 目标

- **双击行的任意位置**（文字或空白）都触发跳转，与点在文字上行为一致。
- **跳转后目标行持续高亮并淡出**（约 1.5 秒，末段约 0.3 秒透明度递减到 0），让用户看清目标行。
- terminal 与 log 两个面板都改，行为一致。

## 设计

### 改动一：双击判定扩到整行任意位置

在 `render_rows_view` 内（`terminal.rs:1602-1610`、`log.rs:756-762`），把 `ctx_response.double_clicked()` 替换为「整行命中」判定：

```rust
// terminal.rs 现状（1602-1610）：
let double_clicked = ctx_response.double_clicked();
let mut pending_navigate: Option<u64> = None;
if double_clicked
    && let Some(idx) = frozen_row_idx.or_else(|| hl.hover_index(ui))
    && let Some(row) = rows.get(idx)
{
    pending_navigate = Some(row.id);
}
```

改为用 egui 全局双击事件 + 行命中（不依赖文本 response）：

```rust
// 双击任意位置（文字或空白）→ 跳转。
// 用全局 button_double_clicked + RowHighlight 行命中，对文字区/空白区一视同仁。
let double_clicked = ui
    .input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary))
    && ui.rect_contains_pointer(label_rect); // 仅当双击落在整个行区域内
let mut pending_navigate: Option<u64> = None;
if double_clicked
    && let Some(idx) = frozen_row_idx.or_else(|| hl.hover_index(ui))
    && let Some(row) = rows.get(idx)
{
    pending_navigate = Some(row.id);
}
```

`label_rect` 是整行可交互区域（已存在，`handle_input` 也用它做 interaction_rect）。`hl.hover_index` 复用现有行命中逻辑。`frozen_row_idx`（右键冻结）优先，回退 `hover_index`，与现状一致。

log.rs 同构替换（`ctx_response` 在 log 里同样只覆盖文本列；`label_rect`/`hl` 均存在）。

注意：`ctx_response` 本身保留，仍用于右键菜单（`context_menu_opened` / `clicked_by(Secondary)`）等，不删。

### 改动二：跳转后目标行高亮 + 淡出

#### 状态

`TerminalPanel` 与 `LogPanel` 各新增临时高亮状态：

```rust
/// 跳转目标行高亮：(目标行 id, 起始时间秒)。
/// 渲染时若该行 id 命中且未超时，画一层强调色背景并按剩余时间淡出。
navigate_highlight: Option<(u64, f64)>,
```

- 跳转触发时设置：在 `apply_render_outcome`（terminal）`terminal.rs:794`、log 对应处拿到 `outcome.pending_navigate_to_id` 时，若 `Some(id)`，设 `navigate_highlight = Some((id, ui.ctx().input(|i| i.time)))`。
- 时间源：`egui::Context::input(|i| i.time)`（秒），无需注入 `tool_core::Clock`。

#### 渲染

`render_rows_view` 增加 `navigate_highlight: Option<(u64, f64)>` 参数（透传 `self.navigate_highlight`）。每行渲染时，在 selection 高亮之后、文字之前，若命中目标行且未超时，画强调色背景：

```rust
const NAV_HIGHLIGHT_DURATION: f64 = 1.5; // 秒
const NAV_FADE: f64 = 0.3;               // 末段淡出时长

if let Some((target_id, start)) = navigate_highlight {
    if row.id == target_id {
        let now = ui.ctx().input(|i| i.time);
        let elapsed = now - start;
        if elapsed < NAV_HIGHLIGHT_DURATION {
            let alpha = if elapsed > NAV_HIGHLIGHT_DURATION - NAV_FADE {
                // 末段线性淡出 1.0 → 0.0
                ((NAV_HIGHLIGHT_DURATION - elapsed) / NAV_FADE).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let color = theme::NAV_HIGHLIGHT.gamma_multiply(alpha as f32); // 半透明强调色
            ui.painter_at(full_rect).rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(full_rect.left(), current_y),
                    egui::vec2(full_rect.width(), entry_height),
                ),
                0.0,
                color,
            );
        }
    }
}
```

`theme::NAV_HIGHLIGHT` 是新增的一个半透明强调色（例如蓝色 ~0.25 alpha），加在 `crates/panels/src/theme.rs`。叠在 selection/hover 之上（绘制顺序在后），文字仍清晰可读。

#### 超时清理

超时检查放在 `apply_render_outcome`（self 层，能拿到 `ui.ctx().input(|i| i.time)`）：若 `navigate_highlight` 的 `elapsed >= DURATION`，置 `None`。淡出期间需 `request_repaint()` 保证动画连续——在 `render_rows_view` 渲染分支命中目标行且未结束时调 `ui.ctx().request_repaint()`。

### 不改的部分

- `RowSelection`、`RowHighlight`（`table.rs`）不动。
- `scroll_to_rect` 跳转逻辑不动。
- `pending_navigate_to_id` 的「下一帧清搜索/筛选/自动滚动」流程不动——只把触发源从「文本双击」换成「整行双击」，并新增高亮。
- 右键菜单、Ctrl+A/C、单击/拖拽选行（上一个 feature 的改动）不动。
- 高亮是临时态，不持久化、不入配置。

## 影响面

- `crates/panels/src/terminal.rs`：双击判定改 ~5 行；新增 `navigate_highlight` 字段 + `apply_render_outcome` 设值 + 渲染分支 ~20 行。
- `crates/panels/src/log.rs`：同构。
- `crates/panels/src/theme.rs`：新增 `NAV_HIGHLIGHT` 常量。
- 无 `table.rs` 改动、无配置项、无持久化。

## 测试

手动验证（terminal 与 log 各一遍）：

1. 双击行**文字** → 跳转 + 目标行高亮淡出（约 1.5s）。
2. 双击行**空白**（行尾 padding / 行间 / 元数据列）→ 同样跳转 + 高亮（回归验证新行为）。
3. 搜索状态下双击结果 → 离开搜索进入上下文，目标行高亮淡出。
4. 行很多、滚动后双击远处行 → 跳转后能凭高亮看清目标行。
5. 高亮淡出结束后不再重绘（无持续 CPU），点击别处不影响。
6. 单击/拖拽选行、右键菜单、Ctrl+A/C → 回归正常。

淡出时长观感：1.5s 是否合适，可在实现后微调常量。`RowSelection` 单测不受影响。双击/高亮依赖 egui 行为，靠手动 + egui inspection MCP 验证，不写纯单测。

## 不做（YAGNI）

- 不改 `RowSelection` / `RowHighlight` 内部状态机。
- 不持久化高亮状态。
- 不把高亮做成可配置颜色/时长（先用常量）。
- 不引入 `Clock` 依赖注入，直接用 `egui::input(|i| i.time)`。

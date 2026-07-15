# 行选中：点击文字即选行，拖动文字即文本选区

## 背景

接收区（Terminal）与日志区（Log）的每一行同时存在两套“选中”：

- **整行多选**：自写的 `RowSelection`（`crates/panels/src/table.rs`），选中对象是整行（稳定 row id）。
- **字符级文本选区**：egui 内置 `LabelSelectionState`，选中对象是行内文本字符。

当前判定按“指针落在哪个矩形”区分（见 `terminal.rs:1487-1514`、`log.rs:663-691`）：

- `row_text_rect` 只裹住 galley 真实文本框。指针落在文本内 → egui 字符级拖选。
- 指针落在文本外空白（行尾 padding、文本上下、元数据列）→ `begin_pointer` 整行选中。

因此**点击文字本身无法选中整行**——点文字总是直接进入字符级选区。

## 目标

改为按**鼠标动作类型**区分，而非按“落点是否在文本矩形内”区分：

- **在文字上单击**（按下 → 原地松开，未拖动）→ 整行选中。
- **在文字上拖动**（按下 → 移动超过阈值 → 松开）→ 字符级文本选区，行为不变。

判定时机：**松开时**。按下时不立即改变选区（无闪烁），松开时若未拖动则整行选中，拖动过则保留字符选区。

修饰键保持现有语义：在文字上 Ctrl/Shift/Ctrl+Shift 单击（未拖动松开）同样触发整行多选（toggle / 范围 / 追加范围），与在空白处单击行为一致。

## 前提验证（已确认）

egui 0.35 对 `Sense::click_and_drag()` 的 widget，click 与 drag 的判定（`egui-0.35.0/src/interaction.rs:174-207`）：

- `Released { click, .. }` 时，仅当 `click.is_some() && !is_decidedly_dragging()` 才置 `clicked`。
- 对同时 senses_click && senses_drag 的 widget，`is_dragged` 取决于 `is_decidedly_dragging()`——指针移动超过 click 阈值才算 drag。

因此：拖动超过阈值后松开 → `response.clicked()` 为 false（走 drag，字符选区正常）；原地按下松开 → `response.clicked()` 为 true。两者天然分开，无需自写拖动阈值。

## 设计

### 改动位置

仅 `crates/panels/src/terminal.rs` 与 `crates/panels/src/log.rs`，各新增一个 `response.clicked()` 分支。`table.rs` 的 `RowSelection` 不动。

### Terminal（`terminal.rs`，当前 1504-1514）

现状（注意：`ui.interact(...)` 当前位于 `primary_pressed` 分支**之后**）：

```rust
let (primary_pressed, ctrl, shift) = ui.input(|i| (
    i.pointer.button_pressed(egui::PointerButton::Primary),
    i.modifiers.ctrl || i.modifiers.command,
    i.modifiers.shift,
));
if primary_pressed
    && ui.rect_contains_pointer(hex_row_rect)
    && !ui.rect_contains_pointer(row_text_rect)
{
    selection.begin_pointer(row_idx, ctrl, shift);
    ui.ctx().plugin::<LabelSelectionState>().lock().clear_selection();
}
let row_id = ui.make_persistent_id(("hex", row.id));
let response = ui.interact(row_text_rect, row_id, Sense::click_and_drag());
```

改为：保留空白分支（按下即选），新增文本内 `clicked()` 分支（松开判定）。注意 `response` 需在 `clicked()` 判定之前构造，把 `ui.interact(...)` 提到分支之前。

```rust
let row_id = ui.make_persistent_id(("hex", row.id));
let response = ui.interact(row_text_rect, row_id, Sense::click_and_drag());

let (primary_pressed, ctrl, shift) = ui.input(|i| (
    i.pointer.button_pressed(egui::PointerButton::Primary),
    i.modifiers.ctrl || i.modifiers.command,
    i.modifiers.shift,
));
// 文本外空白：按下即选（即时反馈）
if primary_pressed
    && ui.rect_contains_pointer(hex_row_rect)
    && !ui.rect_contains_pointer(row_text_rect)
{
    selection.begin_pointer(row_idx, ctrl, shift);
    ui.ctx().plugin::<LabelSelectionState>().lock().clear_selection();
}
// 文本内：松开且未拖动 → 整行选中
if response.clicked() && ui.rect_contains_pointer(row_text_rect) {
    selection.begin_pointer(row_idx, ctrl, shift);
    ui.ctx().plugin::<LabelSelectionState>().lock().clear_selection();
}
```

### Log（`log.rs`，当前 675-691）

同构替换：`hex_row_rect` → `msg_row_rect`，id salt `("hex", row.id)` → `("log-msg", entry.id)`，rect 变量名随之调整。同样把 `ui.interact(...)` 提到 `clicked()` 判定之前。

### 互斥保证

`response.clicked()` 触发整行选中时，同样调用 `LabelSelectionState::lock().clear_selection()` 清掉 egui 的字符级选区。按下瞬间 egui 可能在该 galley 上启动了字符选区“候选”，松开判定为 click 后两者会短暂共存；清掉文本选区即可保证互斥，与现有空白分支做法一致。

### 不改的部分

- `RowSelection`（`table.rs`）：`begin_pointer` / `handle_input` / 拖拽框选语义不变。空白区仍是 `handle_input` 的按下即选。
- `LabelSelectionState::label_text_selection`：继续负责拖选文字。
- 双击整行（`ctx_response.double_clicked()`）、右键菜单行匹配（`RowHighlight`）、Ctrl+A / Ctrl+C 不动。

## 影响面

仅 `terminal.rs`、`log.rs` 各加一个 `clicked()` 分支并把对应 `ui.interact(...)` 提前。无新增结构、无 `table.rs` 改动、无配置项、无持久化。风险低。

## 测试

手动验证（终端与日志各一遍）：

1. 点文字（按下→原地松开）→ 整行选中，无字符选区。
2. 在文字上拖动 → 字符级文本选区，整行不被选中。
3. Ctrl 单击文字 → toggle 该行（多选）。
4. Shift 单击文字 → 从 anchor 到该行范围选择。
5. Ctrl+Shift 单击文字 → 追加范围。
6. 空白处单击 → 行选（回归，行为不变）。
7. 空白处拖动 → 框选多行（回归，行为不变）。
8. 双击行 → 整行选中（回归）。
9. Ctrl+A / Ctrl+C → 全选 / 复制选中行（回归）。

`RowSelection` 现有单元测试不受影响（未改动它）。`clicked()` 行为依赖 egui，靠手动 + egui inspection MCP 验证，不写纯单测。

## 不做（YAGNI）

- 不自写拖动阈值，复用 egui 的 `is_decidedly_dragging`。
- 不改 `RowSelection` 内部状态机（不引入 pending/延迟提交机制）。
- 不实现自绘字符选区。

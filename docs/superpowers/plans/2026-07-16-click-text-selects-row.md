# 点击文字即选行 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让接收区(Terminal)和日志区(Log)在文字上单击即选中整行,在文字上拖动仍是字符级文本选区。

**Architecture:** 在每行文本的 `ui.interact(..., Sense::click_and_drag())` 返回的 `response` 上,新增一个 `response.clicked()` 分支(松开且未拖动 → `begin_pointer` 整行选中 + 清除 egui 字符选区)。复用 egui 0.35 内置的 click/drag 判定,不改 `RowSelection`。

**Tech Stack:** Rust 2024, egui 0.35, `LabelSelectionState`, `RowSelection`(`crates/panels/src/table.rs`)。

**Spec:** `docs/superpowers/specs/2026-07-16-click-text-selects-row-design.md`

---

## File Structure

| 文件 | 职责 | 改动 |
|------|------|------|
| `crates/panels/src/terminal.rs` | 接收区行渲染,文本列交互 | 把 `ui.interact(...)` 提到分支前,新增 `response.clicked()` 整行选中分支 |
| `crates/panels/src/log.rs` | 日志区行渲染,消息列交互 | 同构改动 |
| `crates/panels/src/table.rs` | `RowSelection` 共享抽象 | **不改** |

两处改动结构同构,只是 rect 变量名 / id salt 不同。terminal 用 `hex_row_rect` / `row_text_rect` / `("hex", row.id)`;log 用 `msg_row_rect` / `row_text_rect` / `("log-msg", entry.id)`。

---

### Task 1: Terminal 文本列新增 clicked() 整行选中分支

**Files:**
- Modify: `crates/panels/src/terminal.rs:1482-1521`

**现状(改动前)的精确代码**(`terminal.rs:1495-1517`):

```rust
                    // 文本外空白处按下 → 整行选中（与点元数据区等效）。
                    // drag/release 由 handle_input 接管（label_rect 入口），这里只触发 begin。
                    let (primary_pressed, ctrl, shift) = ui.input(|i| {
                        (
                            i.pointer.button_pressed(egui::PointerButton::Primary),
                            i.modifiers.ctrl || i.modifiers.command,
                            i.modifiers.shift,
                        )
                    });
                    if primary_pressed
                        && ui.rect_contains_pointer(hex_row_rect)
                        && !ui.rect_contains_pointer(row_text_rect)
                    {
                        selection.begin_pointer(row_idx, ctrl, shift);
                        // 整行选中与字符级文本选区互斥：清掉 egui 的 label 文本选区。
                        ui.ctx()
                            .plugin::<LabelSelectionState>()
                            .lock()
                            .clear_selection();
                    }
                    // Use a separate id salt for hex column to avoid id collision with preview
                    let row_id = ui.make_persistent_id(("hex", row.id));
                    let response = ui.interact(row_text_rect, row_id, Sense::click_and_drag());
```

注意:`ui.interact(...)` 当前在空白分支**之后**,但新增的 `response.clicked()` 需要读 `response`,所以必须把 `interact` 提前。

- [ ] **Step 1: 把 `ui.interact(...)` 提前到 `primary_pressed` 分支之前,并新增 `response.clicked()` 分支**

把上面这段替换为:

```rust
                    // 先构造 response：文本外空白分支（按下即选）与文本内 clicked 分支
                    // （松开判定）都要用到它。
                    // Use a separate id salt for hex column to avoid id collision with preview
                    let row_id = ui.make_persistent_id(("hex", row.id));
                    let response = ui.interact(row_text_rect, row_id, Sense::click_and_drag());

                    let (primary_pressed, ctrl, shift) = ui.input(|i| {
                        (
                            i.pointer.button_pressed(egui::PointerButton::Primary),
                            i.modifiers.ctrl || i.modifiers.command,
                            i.modifiers.shift,
                        )
                    });
                    // 文本外空白处按下 → 整行选中（即时反馈，与点元数据区等效）。
                    // drag/release 由 handle_input 接管（label_rect 入口），这里只触发 begin。
                    if primary_pressed
                        && ui.rect_contains_pointer(hex_row_rect)
                        && !ui.rect_contains_pointer(row_text_rect)
                    {
                        selection.begin_pointer(row_idx, ctrl, shift);
                        // 整行选中与字符级文本选区互斥：清掉 egui 的 label 文本选区。
                        ui.ctx()
                            .plugin::<LabelSelectionState>()
                            .lock()
                            .clear_selection();
                    }
                    // 文本内：松开且未拖动 → 整行选中。
                    // response.clicked() 在 egui 中只有"按下→原地松开、未拖动"才为 true
                    // （拖动超过阈值后松开走 drag，clicked 为 false，字符选区正常进行）。
                    // Ctrl/Shift/Ctrl+Shift 修饰键在松开时读取，复用 begin_pointer 语义。
                    if response.clicked() && ui.rect_contains_pointer(row_text_rect) {
                        selection.begin_pointer(row_idx, ctrl, shift);
                        ui.ctx()
                            .plugin::<LabelSelectionState>()
                            .lock()
                            .clear_selection();
                    }
```

紧接其后的两行**保持不变**(不要动):
```rust
                    text_drag_response = Some(match text_drag_response.take() {
                        Some(accumulated) => accumulated | response.clone(),
                        None => response.clone(),
                    });
```

- [ ] **Step 2: 编译 + clippy**

Run: `cargo clippy -p tool-panels --all-targets 2>&1 | tail -20`
Expected: `Finished` 无 error 无新 warning。`_data_pressed` 等既有 dead_code allow 不受影响。

- [ ] **Step 3: 暂不提交,先做 Task 2,一起提交**

(本任务无独立提交,与 Task 2 同属一个语义改动。)

---

### Task 2: Log 消息列新增 clicked() 整行选中分支

**Files:**
- Modify: `crates/panels/src/log.rs:663-697`

**现状(改动前)的精确代码**(`log.rs:675-693`):

```rust
                    let (primary_pressed, ctrl, shift) = ui.input(|i| {
                        (
                            i.pointer.button_pressed(egui::PointerButton::Primary),
                            i.modifiers.ctrl || i.modifiers.command,
                            i.modifiers.shift,
                        )
                    });
                    if primary_pressed
                        && ui.rect_contains_pointer(msg_row_rect)
                        && !ui.rect_contains_pointer(row_text_rect)
                    {
                        selection.begin_pointer(row_idx, ctrl, shift);
                        ui.ctx()
                            .plugin::<LabelSelectionState>()
                            .lock()
                            .clear_selection();
                    }
                    let row_id = ui.make_persistent_id(("log-msg", entry.id));
                    let response = ui.interact(row_text_rect, row_id, Sense::click_and_drag());
```

- [ ] **Step 1: 把 `ui.interact(...)` 提前,并新增 `response.clicked()` 分支**

替换为:

```rust
                    // 先构造 response：文本外空白分支（按下即选）与文本内 clicked 分支
                    // （松开判定）都要用到它。
                    let row_id = ui.make_persistent_id(("log-msg", entry.id));
                    let response = ui.interact(row_text_rect, row_id, Sense::click_and_drag());

                    let (primary_pressed, ctrl, shift) = ui.input(|i| {
                        (
                            i.pointer.button_pressed(egui::PointerButton::Primary),
                            i.modifiers.ctrl || i.modifiers.command,
                            i.modifiers.shift,
                        )
                    });
                    // 文本外空白处按下 → 整行选中（即时反馈）。
                    if primary_pressed
                        && ui.rect_contains_pointer(msg_row_rect)
                        && !ui.rect_contains_pointer(row_text_rect)
                    {
                        selection.begin_pointer(row_idx, ctrl, shift);
                        ui.ctx()
                            .plugin::<LabelSelectionState>()
                            .lock()
                            .clear_selection();
                    }
                    // 文本内：松开且未拖动 → 整行选中。
                    // response.clicked() 只有"按下→原地松开、未拖动"才为 true
                    // （拖动超过阈值后松开走 drag，clicked 为 false，字符选区正常）。
                    if response.clicked() && ui.rect_contains_pointer(row_text_rect) {
                        selection.begin_pointer(row_idx, ctrl, shift);
                        ui.ctx()
                            .plugin::<LabelSelectionState>()
                            .lock()
                            .clear_selection();
                    }
```

紧接其后**保持不变**:
```rust
                    text_drag_response = Some(match text_drag_response.take() {
                        Some(accumulated) => accumulated | response.clone(),
                        None => response.clone(),
                    });
                    ctx_response |= response.clone();
```

- [ ] **Step 2: 全 workspace 编译 + clippy + 测试**

Run: `cargo clippy --all-targets 2>&1 | tail -20 && cargo test --all-targets 2>&1 | tail -25`
Expected: clippy `Finished` 无新 warning;测试全过(`table.rs` 的 `RowSelection` 单测未改动,应原样通过)。

- [ ] **Step 3: 提交 Task 1 + Task 2**

```bash
git add crates/panels/src/terminal.rs crates/panels/src/log.rs
```

Commit message(用 `git commit -F` 传文件,避免 PowerShell here-string 问题):

```
feat(panels): 点击文字即选中整行，拖动文字为字符级选区

终端/日志原按指针是否落在文本矩形内区分整行选/字符选，导致点击文字
无法选中整行。改为按鼠标动作：文本内 response.clicked()（松开且未拖动）
→ 整行选中，拖动 → 字符级文本选区。判定在松开时，按下不改变选区，无闪烁。
复用 egui 0.35 click/drag 判定，RowSelection 不变；terminal.rs 与 log.rs
各把 interact 提前并新增 clicked() 分支。

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
```

---

### Task 3: 手动验证

**Files:** 无文件改动,纯运行验证。

- [ ] **Step 1: 构建运行版**

Run: `cargo build -p hardware-workbench-app 2>&1 | tail -5`
Expected: `Finished`。

- [ ] **Step 2: 运行应用并手动验证(终端 + 日志各一遍)**

Run: `cargo run`（或用户已运行则直接操作）

逐项验证(对应 spec 测试清单):

1. 终端:点文字(按下→原地松开)→ 整行选中,无字符选区。
2. 终端:在文字上拖动 → 字符级文本选区,整行未被选中。
3. 终端:Ctrl 单击文字 → toggle 该行。
4. 终端:Shift 单击文字 → 从 anchor 到该行范围。
5. 终端:空白处单击 → 行选(回归)。
6. 终端:空白处拖动 → 框选多行(回归)。
7. 终端:双击行 → 整行选中(回归)。
8. 日志:重复 1-7。

预期全部符合。任一项不符 → 回到对应 Task 修正,不要跳过。

- [ ] **Step 3: (可选)egui inspection MCP 验证**

若应用以 `EGUI_INSPECTION` 启动,用 `mcp__egui__attach` + `query_tree`/`screenshot` 观察点击文字后行高亮是否出现、字符选区是否被清除。否则跳过,手动验证已足够。

- [ ] **Step 4: 无额外提交(本任务无代码改动)**

---

## Self-Review(计划作者自查)

**1. Spec 覆盖:** spec 的"设计"两处代码替换 → Task 1、Task 2。"互斥保证"(clicked 时 clear_selection)→ 两任务代码块均含。"不改的部分" → 计划明确 table.rs 不改,双击/右键/Ctrl+A/C 不在本计划范围(回归验证)。测试清单 → Task 3 全部覆盖 9 项。无遗漏。

**2. 占位符扫描:** 无 TBD/TODO;每个改动步骤含完整 before/after 代码;提交信息完整。

**3. 类型一致性:** `response.clicked()`、`ui.rect_contains_pointer(row_text_rect)`、`selection.begin_pointer(row_idx, ctrl, shift)`、`LabelSelectionState::lock().clear_selection()` 在两任务中签名一致,且与现有代码用法一致。terminal 用 `hex_row_rect`,log 用 `msg_row_rect` —— 与各自文件现状一致,未混淆。

# 双击任意位置跳转 + 目标行高亮淡出 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让接收区(Terminal)和日志区(Log)双击行的任意位置(文字或空白)都触发跳转,且跳转后目标行持续高亮并淡出约 1.5 秒。

**Architecture:** 双击判定从"文本列 `ctx_response.double_clicked()`"改为"全局双击 + `RowHighlight` 整行命中"。新增 `navigate_highlight: Option<(u64, f64)>` 临时高亮状态(目标行 id + 起始秒),在跳转发生的帧设值,渲染时按剩余时间画半透明强调色背景并淡出。terminal 与 log 同构,`table.rs` 不变。

**Tech Stack:** Rust 2024, egui 0.35(`Color32::gamma_multiply` 做 alpha 淡出,`input(|i| i.time)` 做时间源), `RowHighlight`(`table.rs`)。

**Spec:** `docs/superpowers/specs/2026-07-16-double-click-navigate-highlight-design.md`

---

## File Structure

| 文件 | 职责 | 改动 |
|------|------|------|
| `crates/panels/src/theme.rs` | 颜色常量 | 新增 `NAV_HIGHLIGHT` 半透明强调色 |
| `crates/panels/src/terminal.rs` | 接收区:双击判定 + 高亮状态 + 渲染 | 改双击判定;加 `navigate_highlight` 字段;设值+超时清理;渲染分支 |
| `crates/panels/src/log.rs` | 日志区:同构 | 同构改动 |
| `crates/panels/src/table.rs` | `RowSelection`/`RowHighlight` | **不改** |

### 关键时序(实现前必读)

跳转是**跨帧**的:
1. **帧 A**(双击发生):`render_rows_view`/`render_log_rows` 内检测双击 → 产出 `navigate_id` → `apply_render_outcome`/`ui` 方法把它存进 `self.pending_navigate_to_id`。
2. **帧 B**(跳转生效):`ui` 方法顶部 `pending_navigate_to_id.take()` → `scroll_to_row` → `render_rows_view` 内 `scroll_to_rect` 滚到目标行。

用户在**帧 B** 看到跳转。所以高亮应在**帧 B** 设值(目标行已进入视口),即 `pending_navigate_to_id` 被 consume 的地方:
- terminal: `terminal.rs:756` 附近 `if let Some(target_id) = self.pending_navigate_to_id.take()`
- log: `log.rs:281-284` `self.pending_navigate_to_id.take().and_then(...)`

高亮渲染发生在 `render_rows_view`/`render_log_rows` 的行循环里,需要拿到 `navigate_highlight`。透传方式:作为新参数 `navigate_highlight: Option<(u64, f64)>` 传入(by value,只读)。

超时清理在 `ui` 方法末尾(self 层有 `ui.ctx().input(|i| i.time)`):若已超 `DURATION` 则置 `None`。

---

### Task 1: theme.rs 新增 NAV_HIGHLIGHT 常量

**Files:**
- Modify: `crates/panels/src/theme.rs:76` (在分隔线常量之后)

- [ ] **Step 1: 新增 NAV_HIGHLIGHT 常量**

在 `theme.rs` 的 `SEPARATOR_STRONG` 那行之后(第 76 行后)插入:

```rust
pub const SEPARATOR_STRONG: Color32 = Color32::from_rgb(70, 82, 100);

/// 跳转目标行高亮色（半透明蓝，叠在 selection/hover 之上，文字仍可读）。
pub const NAV_HIGHLIGHT: Color32 = Color32::from_rgba_premultiplied(80, 140, 210, 70);
```

(用 `from_rgba_premultiplied` 直接给 alpha=70/255 ≈ 0.27,无需运行时再乘。`gamma_multiply` 在渲染时用于淡出。)

- [ ] **Step 2: 编译验证**

Run: `cargo clippy -p tool-panels --all-targets 2>&1 | tail -10`
Expected: `Finished`,无 error 无新 warning(常量暂未被引用,可能产生 dead_code warning——这是预期的,Task 2/3 会用到;若 clippy 报 dead_code 可忽略,或在本任务不单独验证、留到 Task 3 一起验证)。

- [ ] **Step 3: 不单独提交,与后续任务一起提交**

---

### Task 2: Terminal 双击任意位置 + 高亮

**Files:**
- Modify: `crates/panels/src/terminal.rs`

本任务有 4 处改动,按顺序执行。

#### 改动 2a: `TerminalPanel` 新增 `navigate_highlight` 字段

`TerminalPanel` 结构体定义(约 `terminal.rs:56` 附近,与 `pending_navigate_to_id: Option<u64>` 同区)。

- [ ] **Step 1: 在 `pending_navigate_to_id` 字段后加 `navigate_highlight`**

找到(`terminal.rs:56` 附近):
```rust
    pending_navigate_to_id: Option<u64>,
```
在其后加:
```rust
    pending_navigate_to_id: Option<u64>,
    /// 跳转目标行高亮：(目标行 id, 起始时间秒)。渲染时若命中且未超时画强调色并淡出。
    navigate_highlight: Option<(u64, f64)>,
```

- [ ] **Step 2: 在构造函数(`TerminalPanel::new` / 默认初始化,约 `terminal.rs:335`)初始化**

找到(`terminal.rs:335` 附近):
```rust
            pending_navigate_to_id: None,
```
在其后加:
```rust
            pending_navigate_to_id: None,
            navigate_highlight: None,
```

#### 改动 2b: 双击判定改为整行命中

`render_rows_view` 内(`terminal.rs:1602-1610`)。

- [ ] **Step 3: 替换双击判定**

找到:
```rust
            // 双击搜索结果 → 离开搜索进入上下文：设置导航目标让下帧跳转
            let double_clicked = ctx_response.double_clicked();
            let mut pending_navigate: Option<u64> = None;
            if double_clicked
                && let Some(idx) = frozen_row_idx.or_else(|| hl.hover_index(ui))
                && let Some(row) = rows.get(idx)
            {
                pending_navigate = Some(row.id);
            }
```
替换为:
```rust
            // 双击任意位置（文字或空白）→ 离开搜索进入上下文：设置导航目标让下帧跳转。
            // 用全局 button_double_clicked + 整行 rect 命中，不再依赖只覆盖文本列的 ctx_response。
            let double_clicked = ui
                .input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary))
                && ui.rect_contains_pointer(label_rect);
            let mut pending_navigate: Option<u64> = None;
            if double_clicked
                && let Some(idx) = frozen_row_idx.or_else(|| hl.hover_index(ui))
                && let Some(row) = rows.get(idx)
            {
                pending_navigate = Some(row.id);
            }
```

(`label_rect` 是整行可交互区域,`render_rows_view` 内已存在;`hl.hover_index` 复用现有行命中。)

#### 改动 2c: 高亮渲染分支

`render_rows_view` 需要 `navigate_highlight`。先加参数,再在行循环里画。

- [ ] **Step 4: 给 `render_rows_view` 加参数**

签名(`terminal.rs:1150-1170`)加一个参数(放在 `cached_total_height_rows` 之后、`-> RenderOutcome` 之前):
```rust
    cached_total_height: &mut f32,
    cached_total_height_rows: &mut usize,
    navigate_highlight: Option<(u64, f64)>,
) -> RenderOutcome {
```

调用处(`terminal.rs:768`, `render_rows_view(ui, ...)`)末尾加传参。找到调用:
```rust
                &mut self.cached_total_height,
                &mut self.cached_total_height_rows,
            )
        };
```
改为:
```rust
                &mut self.cached_total_height,
                &mut self.cached_total_height_rows,
                self.navigate_highlight,
            )
        };
```

- [ ] **Step 5: 在行循环里画高亮**

在 `render_rows_view` 行循环中,找到选中高亮之后、画文字之前的位置(`terminal.rs:1439-1442`):
```rust
                // 框选高亮（使用 WIDGET_HOVER 颜色，与 hover 一致）
                if selection.is_selected(row_idx) {
                    selection.paint(ui, full_rect, current_y, entry_height);
                }
```
在其**后**插入高亮分支:
```rust
                // 框选高亮（使用 WIDGET_HOVER 颜色，与 hover 一致）
                if selection.is_selected(row_idx) {
                    selection.paint(ui, full_rect, current_y, entry_height);
                }

                // 跳转目标行高亮（叠在 selection/hover 之上，按剩余时间淡出）
                if let Some((target_id, start)) = navigate_highlight
                    && row.id == target_id
                {
                    let now = ui.ctx().input(|i| i.time);
                    let elapsed = now - start;
                    if elapsed < NAV_HIGHLIGHT_DURATION {
                        let alpha = if elapsed > NAV_HIGHLIGHT_DURATION - NAV_FADE {
                            ((NAV_HIGHLIGHT_DURATION - elapsed) / NAV_FADE).clamp(0.0, 1.0)
                        } else {
                            1.0
                        };
                        ui.painter_at(full_rect).rect_filled(
                            egui::Rect::from_min_size(
                                egui::pos2(full_rect.left(), current_y),
                                egui::vec2(full_rect.width(), entry_height),
                            ),
                            0.0,
                            theme::NAV_HIGHLIGHT.gamma_multiply(alpha as f32),
                        );
                        ui.ctx().request_repaint();
                    }
                }
```

- [ ] **Step 6: 在文件顶部加常量**

在 `terminal.rs` 顶部(`use` 之后,第一个 `const` 或 `struct` 之前,或与现有模块常量同区)加:
```rust
/// 跳转目标行高亮总时长（秒）。
const NAV_HIGHLIGHT_DURATION: f64 = 1.5;
/// 跳转目标行高亮末段淡出时长（秒）。
const NAV_FADE: f64 = 0.3;
```

(已确认 `terminal.rs` 与 `log.rs` 顶部均有 `use crate::{ ... theme }`,故 `theme::NAV_HIGHLIGHT` 直接可用,无需额外 use。常量放在文件顶部 `use` 之后、第一个定义之前,或与现有模块常量同区。)

#### 改动 2d: 设值 + 超时清理(在 `ui` 方法,self 层)

跳转生效帧设值(`terminal.rs:756`),`ui` 方法末尾超时清理。

- [ ] **Step 7: 跳转生效时设高亮**

找到(`terminal.rs:755-758`):
```rust
            // 获取下帧跳转目标的 row 索引（现在是完整列表）
            if let Some(target_id) = self.pending_navigate_to_id.take() {
                scroll_to_row = rows.iter().position(|r| r.id == target_id);
            }
```
改为:
```rust
            // 获取下帧跳转目标的 row 索引（现在是完整列表）
            if let Some(target_id) = self.pending_navigate_to_id.take() {
                scroll_to_row = rows.iter().position(|r| r.id == target_id);
                // 跳转生效：设置目标行高亮（起始时间用 egui 时钟）。
                self.navigate_highlight = Some((target_id, ui.ctx().input(|i| i.time)));
            }
```

- [ ] **Step 8: 超时清理**

在 `ui` 方法末尾(`self.apply_render_outcome(...)` 之后,`self.detail_popup(...)` 之前或之后均可,`terminal.rs:790-791` 附近)加:
```rust
        // 高亮超时清理
        if let Some((_, start)) = self.navigate_highlight {
            let now = ui.ctx().input(|i| i.time);
            if now - start >= NAV_HIGHLIGHT_DURATION {
                self.navigate_highlight = None;
            }
        }
```

- [ ] **Step 9: 编译 + clippy + 测试**

Run: `cargo clippy -p tool-panels --all-targets 2>&1 | tail -15 && cargo test -p tool-panels 2>&1 | tail -15`
Expected: `Finished`,无 error 无新 warning;测试全过(table.rs 未改)。

- [ ] **Step 10: 不单独提交,与 Task 3 一起提交**

---

### Task 3: Log 双击任意位置 + 高亮(同构)

**Files:**
- Modify: `crates/panels/src/log.rs`

与 Task 2 同构,4 处改动。

#### 改动 3a: `LogPanel` 加字段

- [ ] **Step 1: 加 `navigate_highlight` 字段**

`log.rs:34` 附近:
```rust
    pending_navigate_to_id: Option<u64>,
```
后加:
```rust
    pending_navigate_to_id: Option<u64>,
    /// 跳转目标行高亮：(目标行 id, 起始时间秒)。渲染时若命中且未超时画强调色并淡出。
    navigate_highlight: Option<(u64, f64)>,
```

- [ ] **Step 2: 初始化**

`log.rs:76` 附近:
```rust
            pending_navigate_to_id: None,
```
后加:
```rust
            pending_navigate_to_id: None,
            navigate_highlight: None,
```

#### 改动 3b: 双击判定改为整行命中

- [ ] **Step 3: 替换双击判定**

`log.rs:755-762`:
```rust
            // 双击搜索结果 → 离开搜索进入上下文
            let double_clicked = ctx_response.double_clicked();
            if double_clicked
                && let Some(idx) = frozen_row_idx.or_else(|| hl.hover_index(ui))
                && let Some(entry) = rows.get(idx)
            {
                *pending_navigate = Some(entry.id);
            }
```
替换为:
```rust
            // 双击任意位置（文字或空白）→ 离开搜索进入上下文。
            // 用全局 button_double_clicked + 整行 rect 命中，不再依赖只覆盖文本列的 ctx_response。
            let double_clicked = ui
                .input(|i| i.pointer.button_double_clicked(egui::PointerButton::Primary))
                && ui.rect_contains_pointer(label_rect);
            if double_clicked
                && let Some(idx) = frozen_row_idx.or_else(|| hl.hover_index(ui))
                && let Some(entry) = rows.get(idx)
            {
                *pending_navigate = Some(entry.id);
            }
```

(`label_rect` 在 `render_log_rows` 内已存在;`hl.hover_index` 复用。)

#### 改动 3c: 高亮渲染分支

- [ ] **Step 4: 给 `render_log_rows` 加参数**

签名(`log.rs:427-438`)加参数(`selection: &mut RowSelection,` 之后):
```rust
    selection: &mut RowSelection,
    navigate_highlight: Option<(u64, f64)>,
) -> LogRenderOutcome {
```

调用处(`log.rs:288-298`)末尾加传参:
```rust
            &mut self.selection,
            self.navigate_highlight,
        );
```

- [ ] **Step 5: 在行循环里画高亮**

在 `render_log_rows` 行循环中,选中高亮之后(`log.rs:616-618`):
```rust
                // 框选高亮
                if selection.is_selected(row_idx) {
                    selection.paint(ui, full_rect, current_y, entry_height);
                }
```
其后插入(与 terminal 完全相同):
```rust
                // 框选高亮
                if selection.is_selected(row_idx) {
                    selection.paint(ui, full_rect, current_y, entry_height);
                }

                // 跳转目标行高亮（叠在 selection/hover 之上，按剩余时间淡出）
                if let Some((target_id, start)) = navigate_highlight
                    && entry.id == target_id
                {
                    let now = ui.ctx().input(|i| i.time);
                    let elapsed = now - start;
                    if elapsed < NAV_HIGHLIGHT_DURATION {
                        let alpha = if elapsed > NAV_HIGHLIGHT_DURATION - NAV_FADE {
                            ((NAV_HIGHLIGHT_DURATION - elapsed) / NAV_FADE).clamp(0.0, 1.0)
                        } else {
                            1.0
                        };
                        ui.painter_at(full_rect).rect_filled(
                            egui::Rect::from_min_size(
                                egui::pos2(full_rect.left(), current_y),
                                egui::vec2(full_rect.width(), entry_height),
                            ),
                            0.0,
                            theme::NAV_HIGHLIGHT.gamma_multiply(alpha as f32),
                        );
                        ui.ctx().request_repaint();
                    }
                }
```

注意 log 的行变量是 `entry`(不是 `row`),条件用 `entry.id == target_id`。

- [ ] **Step 6: 加常量**

`log.rs` 顶部加(与 terminal 相同):
```rust
/// 跳转目标行高亮总时长（秒）。
const NAV_HIGHLIGHT_DURATION: f64 = 1.5;
/// 跳转目标行高亮末段淡出时长（秒）。
const NAV_FADE: f64 = 0.3;
```

(已确认 `log.rs` 顶部有 `use crate::{ ... theme }`,`theme::NAV_HIGHLIGHT` 直接可用。)

#### 改动 3d: 设值 + 超时清理

- [ ] **Step 7: 跳转生效时设高亮**

`log.rs:280-284`:
```rust
        // 获取跳转目标的 row 索引
        let scroll_to_row: Option<usize> = self
            .pending_navigate_to_id
            .take()
            .and_then(|target_id| rows.iter().position(|entry| entry.id == target_id));
```
改为:
```rust
        // 获取跳转目标的 row 索引
        let scroll_to_row: Option<usize> = self
            .pending_navigate_to_id
            .take()
            .and_then(|target_id| {
                // 跳转生效：设置目标行高亮（起始时间用 egui 时钟）。
                self.navigate_highlight = Some((target_id, ui.ctx().input(|i| i.time)));
                rows.iter().position(|entry| entry.id == target_id)
            });
```

(注意:`take().and_then` 闭包内 `self` 借用——若编译报借用冲突,改为先 `take` 出来再处理:见 Step 7b 备选。)

**Step 7b(仅在 Step 7 借用冲突时用):**
```rust
        // 获取跳转目标的 row 索引
        let taken_id = self.pending_navigate_to_id.take();
        let scroll_to_row: Option<usize> = taken_id.and_then(|target_id| {
            self.navigate_highlight = Some((target_id, ui.ctx().input(|i| i.time)));
            rows.iter().position(|entry| entry.id == target_id)
        });
```
若 Step 7 直接编译通过则跳过 7b。

- [ ] **Step 8: 超时清理**

在 log 的 `ui` 方法末尾(`if navigate_id.is_some() { ... }` 之后,`self.update_auto_scroll(...)` 之前或之后,`log.rs:300-302` 附近)加:
```rust
        // 高亮超时清理
        if let Some((_, start)) = self.navigate_highlight {
            let now = ui.ctx().input(|i| i.time);
            if now - start >= NAV_HIGHLIGHT_DURATION {
                self.navigate_highlight = None;
            }
        }
```

- [ ] **Step 9: 全 workspace 编译 + clippy + 测试**

Run: `cargo clippy --all-targets 2>&1 | tail -15 && cargo test --all-targets 2>&1 | tail -20`
Expected: `Finished`,无 error 无新 warning;测试全过。(`tool-updater` 测试若报 Windows UAC 提升错误,属环境问题,与改动无关,忽略。)

- [ ] **Step 10: 提交 Task 1+2+3**

```bash
git add crates/panels/src/theme.rs crates/panels/src/terminal.rs crates/panels/src/log.rs
```

Commit message(用 `git commit -F` 传文件):

```
feat(panels): 双击任意位置跳转 + 目标行高亮淡出

双击跳转原用文本列 response 的 double_clicked，空白不触发；跳转后只
滚动无高亮，行多看不清。改为全局双击 + RowHighlight 整行命中，文字/
空白均可触发。新增 navigate_highlight 临时高亮状态（约1.5s，末段淡出），
跳转生效帧设值、超时清理。terminal 与 log 同构，table.rs 不变。

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
```

---

### Task 4: 手动验证

**Files:** 无文件改动。

- [ ] **Step 1: 构建运行版**

Run: `cargo build -p hardware-workbench-app 2>&1 | tail -5`
Expected: `Finished`。(若报 exe 拒绝访问,先结束运行中的 app 进程再重试。)

- [ ] **Step 2: 运行并手动验证(terminal + log 各一遍)**

Run: `cargo run`

逐项验证:
1. 双击行**文字** → 跳转 + 目标行高亮淡出(约 1.5s,末段透明度递减)。
2. 双击行**空白**(行尾 padding / 行间 / 元数据列)→ 同样跳转 + 高亮(新行为)。
3. 搜索状态下双击结果 → 离开搜索进入上下文,目标行高亮淡出。
4. 行很多、滚动后双击远处行 → 跳转后能凭高亮看清目标行。
5. 高亮淡出结束后不再持续重绘(无持续 CPU 占用)。
6. 单击/拖拽选行、右键菜单、Ctrl+A/C → 回归正常(上一个 feature 的改动不受影响)。

任一项不符 → 回到对应 Task 修正,不跳过。淡出时长若观感不合适,微调 `NAV_HIGHLIGHT_DURATION` / `NAV_FADE` 常量。

- [ ] **Step 3: 无额外提交**

---

## Self-Review(计划作者自查)

**1. Spec 覆盖:**
- 改动一(双击整行命中)→ Task 2 Step 3、Task 3 Step 3。✅
- 改动二(高亮状态)→ Task 2 Step 1-2、Task 3 Step 1-2。✅
- 高亮渲染+淡出 → Task 2 Step 5、Task 3 Step 5(含 `gamma_multiply` 淡出、`request_repaint`)。✅
- 超时清理 → Task 2 Step 8、Task 3 Step 8。✅
- 设值时机(跳转生效帧)→ Task 2 Step 7、Task 3 Step 7。✅
- theme.rs `NAV_HIGHLIGHT` → Task 1。✅
- "不改的部分"(table.rs / scroll_to_rect / 右键 / Ctrl+A/C / 不持久化)→ 计划未触及 table.rs,未改 scroll_to_rect,`ctx_response` 保留用于右键。✅
- 测试清单 → Task 4 覆盖 6 项 + spec 的回归项。✅

**2. 占位符扫描:** 无 TBD/TODO。每处改动含完整 before/after 代码。Step 7b 是条件备选(借用冲突时),已给出完整代码,非占位符。常量值明确(1.5/0.3)。`theme` 引用已确认两文件顶部 `use crate::{ ... theme }`,`theme::NAV_HIGHLIGHT` 直接可用。

**3. 类型一致性:** `navigate_highlight: Option<(u64, f64)>` 在字段定义、初始化、参数传递、设值、渲染、超时清理中类型一致。常量 `NAV_HIGHLIGHT_DURATION: f64`/`NAV_FADE: f64` 在两文件一致。`theme::NAV_HIGHLIGHT: Color32` + `.gamma_multiply(f32)` 签名匹配(Color32::gamma_multiply(self, f32) -> Color32)。terminal 用 `row.id`,log 用 `entry.id`——与各自文件行变量名一致,未混淆。`label_rect`、`hl.hover_index`、`frozen_row_idx`、`full_rect`、`current_y`、`entry_height` 均为已确认存在的局部变量。

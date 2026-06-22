# 接收区文本选择 + 原始文本显示 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 将接收区内容区域改为只读 TextEdit 支持文本选择复制，新增"原始"复选框显示控制字符。

**Architecture:** 仅修改 `crates/panels/src/terminal.rs`。`show_entry_multiline()` 重构为 painter 列 + TextEdit 内容区。新增 `show_raw` 字段。

**Tech Stack:** Rust 2024 edition, egui 0.34

## Global Constraints

- 仅修改 `crates/panels/src/terminal.rs`
- `show_raw` 默认 `false`
- 内容区 TextEdit 为只读（`interactive(false)` 或等效方式）
- 每条数据占一行（即使数据本身无 `\n`）
- 时间戳、端口、方向标签保留 painter 绘制
- 详情弹窗"原始内容"显示真正的原始文本

---

### Task 1: 新增 show_raw 字段 + 复选框

**Files:**
- Modify: `crates/panels/src/terminal.rs`

- [ ] **Step 1: 在结构体中新增 show_raw 字段**

在 `show_hex` 之后添加：
```rust
    show_raw: bool,
```

- [ ] **Step 2: 在 new() 中初始化**

在 `show_hex: false,` 之后添加：
```rust
            show_raw: false,
```

- [ ] **Step 3: 在 clear() 中重置**

在 `clear()` 中 `show_port = true;` 之后添加：
```rust
        self.show_raw = false;
```

- [ ] **Step 4: 在 ui() 工具栏添加"原始"复选框**

在 `ui.checkbox(&mut self.show_hex, "HEX");` 之后添加：
```rust
            ui.checkbox(&mut self.show_raw, "原始");
```

- [ ] **Step 5: 在 port_ui() 工具栏添加"原始"复选框**

在 `ui.checkbox(&mut show_hex, "HEX");` 之后添加：
```rust
                ui.checkbox(&mut self.show_raw, "原始");
```

- [ ] **Step 6: 编译检查 + 提交**

```powershell
cargo check -p panels 2>&1
git add crates/panels/src/terminal.rs && git commit -m "feat(terminal): add show_raw field and checkbox"
```

---

### Task 2: 重构 show_entry_multiline 为 painter 列 + TextEdit 内容区

**Files:**
- Modify: `crates/panels/src/terminal.rs`

这是核心改动。将 `show_entry_multiline` 和 `render_rows_view` 重构：

- [ ] **Step 1: 修改 render_rows_view 构建拼接文本**

在 `render_rows_view` 中，将所有可见行的内容拼接成一个多行字符串，每条数据一行：

```rust
// 构建拼接文本：每条数据一行
let combined_text: String = rows
    .iter()
    .map(|row| {
        if show_hex {
            &row.entry.hex_preview
        } else if show_raw {
            &row.entry.raw_text
        } else {
            &row.entry.display_text
        }
    })
    .collect::<Vec<_>>()
    .join("\n");
```

- [ ] **Step 2: 用 TextEdit 渲染内容区**

将 ScrollArea 内的逐行 painter 渲染替换为只读 TextEdit：

```rust
let mut text_copy = combined_text.clone();
let text_edit = egui::TextEdit::multiline(&mut text_copy)
    .desired_width(f32::INFINITY)
    .font(egui::TextStyle::Monospace)
    .interactive(false);
ui.add(text_edit);
```

- [ ] **Step 3: 在左侧绘制时间/端口/方向标签**

在 TextEdit 之前，用 painter 在每行的左侧绘制时间戳、端口、方向标签。这些标签作为"行号/边框"不可选中。

需要计算每行的 y 坐标，在对应位置绘制标签。

- [ ] **Step 4: 处理 show_timestamp/show_port 条件**

标签绘制遵循现有的 `show_timestamp` 和 `show_port` 复选框状态。

- [ ] **Step 5: 编译检查 + 测试 + 提交**

```powershell
cargo check -p panels 2>&1
cargo test -p panels 2>&1
git add crates/panels/src/terminal.rs && git commit -m "feat(terminal): replace painter text with selectable TextEdit"
```

---

### Task 3: 修复详情弹窗原始内容

**Files:**
- Modify: `crates/panels/src/terminal.rs`

- [ ] **Step 1: 详情弹窗"原始内容"显示真正的原始文本**

将 `detail_popup` 中"原始内容"区域的 `detail.raw_text` 改为显示控制字符可视化版本。新增一个辅助函数 `format_raw_visible()` 将 `\n` → `\\n`、`\r` → `\\r`、`\t` → `\\t` 等可视化。

```rust
fn format_raw_visible(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '\0' => output.push_str("\\0"),
            ch if ch.is_control() => output.push_str(&format!("\\x{:02x}", ch as u8)),
            ch => output.push(ch),
        }
    }
    output
}
```

- [ ] **Step 2: 编译检查 + 测试 + 提交**

```powershell
cargo check -p panels 2>&1
cargo test -p panels 2>&1
git add crates/panels/src/terminal.rs && git commit -m "fix(terminal): show raw text with visible control chars in detail popup"
```

---

### Task 4: 运行完整测试套件 + clippy

- [ ] **Step 1: 全量测试**

```powershell
cargo test --all-targets 2>&1
```

- [ ] **Step 2: Clippy**

```powershell
cargo clippy --all-targets 2>&1
```

- [ ] **Step 3: 提交修复（如有）**

```bash
git add -A && git commit -m "chore: clippy fixes"
```

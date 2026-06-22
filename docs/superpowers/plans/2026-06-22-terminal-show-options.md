# 接收区显示选项 — 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在接收区全局视图中添加"显示时间"和"显示端口"复选框，并缩小端口列宽度和列间距。

**Architecture:** 仅修改 `crates/panels/src/terminal.rs` 一个文件。在 `TerminalPanel` 结构体新增两个 `bool` 字段，在 `ui()` 搜索行渲染复选框，在 `show_entry_multiline()` 和导出方法中根据字段条件跳过对应列。常量 `PORT_COL_WIDTH` 和 `COL_GAP` 直接改值。

**Tech Stack:** Rust 2024 edition, egui 0.34

## Global Constraints

- 仅修改 `crates/panels/src/terminal.rs`
- `show_timestamp` 默认 `true`，`show_port` 默认 `true`
- `PORT_COL_WIDTH`: `64.0` → `52.0`
- `COL_GAP`: `4.0` → `3.0`
- 单端口视图只加"显示时间"，不加"显示端口"
- 导出跟随复选框状态
- `clear()` 重置两个字段为默认值

---

### Task 1: 修改常量 + 新增结构体字段

**Files:**
- Modify: `crates/panels/src/terminal.rs:8-12` (常量), `crates/panels/src/terminal.rs:14-38` (结构体)

**Interfaces:**
- Produces: `TerminalPanel.show_timestamp: bool` (默认 `true`), `TerminalPanel.show_port: bool` (默认 `true`), `PORT_COL_WIDTH: f32 = 52.0`, `COL_GAP: f32 = 3.0`

- [ ] **Step 1: 修改常量**

将第 8-12 行的常量修改为：

```rust
const TIME_COL_WIDTH: f32 = 118.0;
const PORT_COL_WIDTH: f32 = 52.0;
const DIR_COL_WIDTH: f32 = 28.0;
const ROW_LEFT_PADDING: f32 = 4.0;
const COL_GAP: f32 = 3.0;
```

- [ ] **Step 2: 在 TerminalPanel 结构体中新增两个字段**

在 `TerminalPanel` 结构体的 `show_hex` 字段之后（第 20 行后）插入：

```rust
    show_timestamp: bool,
    show_port: bool,
```

- [ ] **Step 3: 在 TerminalPanel::new() 中初始化新字段**

在 `new()` 方法中，`show_hex: false,` 之后（第 104 行后）插入：

```rust
            show_timestamp: true,
            show_port: true,
```

- [ ] **Step 4: 编译检查**

```powershell
cargo check -p panels 2>&1
```

预期：编译通过（新字段未使用会有 warning，后续任务消除）。

- [ ] **Step 5: Commit**

```bash
git add crates/panels/src/terminal.rs
git commit -m "feat(terminal): add show_timestamp/show_port fields and tighten column spacing"
```

---

### Task 2: 在 ui() 搜索行添加复选框

**Files:**
- Modify: `crates/panels/src/terminal.rs:381-404` (ui() 方法的搜索行)

**Interfaces:**
- Consumes: `TerminalPanel.show_timestamp: bool`, `TerminalPanel.show_port: bool`

- [ ] **Step 1: 在搜索行添加两个复选框**

将 `ui()` 方法中第 381-404 行的搜索行替换为：

```rust
        ui.horizontal(|ui| {
            ui.label("搜索");
            ui.add(
                egui::TextEdit::singleline(&mut self.search_text)
                    .desired_width(140.0)
                    .hint_text("文本 / HEX"),
            );

            ui.label("端口");
            egui::ComboBox::from_id_salt("terminal-port-filter")
                .width(100.0)
                .selected_text(self.port_filter.as_deref().unwrap_or("全部"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.port_filter, None, "全部");
                    for port in self.ports.keys() {
                        ui.selectable_value(&mut self.port_filter, Some(port.clone()), port);
                    }
                });

            ui.checkbox(&mut self.show_timestamp, "时间");
            ui.checkbox(&mut self.show_port, "端口");

            if ui.button("清除筛选").clicked() {
                self.search_text.clear();
                self.port_filter = None;
            }
        });
```

- [ ] **Step 2: 编译检查**

```powershell
cargo check -p panels 2>&1
```

预期：编译通过。

- [ ] **Step 3: Commit**

```bash
git add crates/panels/src/terminal.rs
git commit -m "feat(terminal): add show timestamp/port checkboxes in search row"
```

---

### Task 3: 在 port_ui() 工具栏添加"显示时间"复选框

**Files:**
- Modify: `crates/panels/src/terminal.rs:280-302` (port_ui() 方法的工具栏)

**Interfaces:**
- Consumes: `TerminalPanel.show_timestamp: bool`

- [ ] **Step 1: 在单端口视图工具栏添加"时间"复选框**

将 `port_ui()` 方法中第 280-302 行的 `ui.horizontal_wrapped` 块替换为：

```rust
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(port_name).monospace().strong());

                ui.checkbox(&mut data.show_rx, "RX");
                ui.checkbox(&mut data.show_tx, "TX");
                ui.checkbox(&mut show_hex, "HEX");
                ui.checkbox(&mut self.show_timestamp, "时间");

                force_scroll_to_bottom |= crate::theme::auto_scroll_button(ui, &mut auto_scroll);

                if ui.button("清空").clicked() {
                    data.entries.clear();
                    clear_selection = true;
                }

                if ui.button("⛶").on_hover_text("放大查看").clicked() {
                    maximize_clicked = true;
                }

                let dropped = self.subscription.dropped_count();
                if dropped > 0 {
                    ui.colored_label(theme::YELLOW, format!("已丢弃 {dropped} 条，数据不完整"));
                }
            });
```

- [ ] **Step 2: 编译检查**

```powershell
cargo check -p panels 2>&1
```

预期：编译通过。

- [ ] **Step 3: Commit**

```bash
git add crates/panels/src/terminal.rs
git commit -m "feat(terminal): add show timestamp checkbox in port view toolbar"
```

---

### Task 4: 在 show_entry_multiline() 中根据字段条件跳过列渲染

**Files:**
- Modify: `crates/panels/src/terminal.rs:933-1023` (show_entry_multiline 函数)

**Interfaces:**
- Consumes: `TerminalPanel.show_timestamp: bool`, `TerminalPanel.show_port: bool`

- [ ] **Step 1: 修改函数签名，新增 show_timestamp 和 show_port 参数**

将 `show_entry_multiline` 函数签名（第 933 行）改为：

```rust
fn show_entry_multiline(
    ui: &mut egui::Ui,
    port: Option<&str>,
    entry: &TerminalEntry,
    show_hex: bool,
    show_timestamp: bool,
    show_port: bool,
    base_row_height: f32,
    selected: bool,
) -> egui::Response {
```

- [ ] **Step 2: 条件渲染时间戳列**

将第 964-974 行的时间戳渲染代码替换为：

```rust
    let mut x = rect.left() + ROW_LEFT_PADDING;

    // 时间戳 — 第一行居中
    if show_timestamp {
        painter.text(
            egui::pos2(x, text_y),
            egui::Align2::LEFT_CENTER,
            &entry.timestamp_label,
            font_id.clone(),
            theme::TEXT_SECONDARY,
        );
        x += TIME_COL_WIDTH + COL_GAP;
    }
```

- [ ] **Step 3: 条件渲染端口列**

将第 976-986 行的端口渲染代码替换为：

```rust
    // 端口 — 第一行居中
    if show_port {
        if let Some(port) = port {
            painter.text(
                egui::pos2(x, text_y),
                egui::Align2::LEFT_CENTER,
                port,
                font_id.clone(),
                theme::YELLOW,
            );
            x += PORT_COL_WIDTH + COL_GAP;
        }
    }
```

- [ ] **Step 4: 更新所有调用点**

`render_rows_view` 函数（第 760 行）签名新增 `show_timestamp: bool` 和 `show_port: bool` 参数：

```rust
fn render_rows_view(
    ui: &mut egui::Ui,
    scroll_key: &str,
    height: f32,
    rows: &[VisibleRow<'_>],
    show_hex: bool,
    show_timestamp: bool,
    show_port: bool,
    stick_to_bottom: bool,
    force_scroll_to_bottom: bool,
    selected_entry_id: Option<u64>,
) -> RenderOutcome {
```

在 `render_rows_view` 中调用 `show_entry_multiline` 处（第 802 行）更新为：

```rust
                let response = show_entry_multiline(
                    ui,
                    row.port,
                    row.entry,
                    show_hex,
                    show_timestamp,
                    show_port,
                    base_row_height,
                    selected,
                );
```

- [ ] **Step 5: 更新 ui() 中对 render_rows_view 的调用**

将 `ui()` 方法中第 455 行的调用更新为：

```rust
            render_rows_view(
                ui,
                &scroll_key,
                scroll_height,
                &rows,
                self.show_hex,
                self.show_timestamp,
                self.show_port,
                self.auto_scroll,
                force_scroll_to_bottom,
                self.selected_entry_id,
            )
```

- [ ] **Step 6: 更新 port_ui() 中对 render_rows_view 的调用**

将 `port_ui()` 方法中第 316 行的调用更新为：

```rust
            render_rows_view(
                ui,
                &scroll_key,
                scroll_height,
                &rows,
                show_hex,
                self.show_timestamp,
                true, // 单端口视图始终显示端口名（工具栏已标明）
                auto_scroll,
                force_scroll_to_bottom,
                self.selected_entry_id,
            )
```

- [ ] **Step 7: 编译检查**

```powershell
cargo check -p panels 2>&1
```

预期：编译通过，无 warning。

- [ ] **Step 8: Commit**

```bash
git add crates/panels/src/terminal.rs
git commit -m "feat(terminal): conditionally render timestamp and port columns"
```

---

### Task 5: 更新导出方法 + clear() 重置

**Files:**
- Modify: `crates/panels/src/terminal.rs:198-258` (导出方法), `crates/panels/src/terminal.rs:150-161` (clear 方法)

**Interfaces:**
- Consumes: `TerminalPanel.show_timestamp: bool`, `TerminalPanel.show_port: bool`

- [ ] **Step 1: 更新 export_visible_csv()**

将 `export_visible_csv` 方法（第 198-225 行）替换为：

```rust
    pub fn export_visible_csv(&self) -> String {
        let show_hex = self.show_hex;
        let show_timestamp = self.show_timestamp;
        let show_port = self.show_port;

        let mut headers: Vec<&str> = Vec::new();
        if show_timestamp { headers.push("time"); }
        if show_port { headers.push("port"); }
        headers.push("direction");
        if show_hex { headers.push("hex"); } else { headers.push("text"); }

        let mut out = headers.join(",");
        out.push('\n');

        for (port, entry) in self.filtered_entries() {
            let mut cells: Vec<String> = Vec::new();
            if show_timestamp {
                cells.push(csv_cell(&entry.timestamp_label));
            }
            if show_port {
                cells.push(csv_cell(&port));
            }
            cells.push(csv_cell(match entry.direction {
                Direction::Rx => "RX",
                Direction::Tx => "TX",
                Direction::Internal => "INTERNAL",
            }));
            if show_hex {
                cells.push(csv_cell(&entry.hex_text));
            } else {
                cells.push(csv_cell(&entry.raw_text));
            }
            out.push_str(&cells.join(","));
            out.push('\n');
        }
        out
    }
```

- [ ] **Step 2: 更新 export_visible_jsonl()**

将 `export_visible_jsonl` 方法（第 227-258 行）替换为：

```rust
    pub fn export_visible_jsonl(&self) -> String {
        let show_hex = self.show_hex;
        let show_timestamp = self.show_timestamp;
        let show_port = self.show_port;

        let mut out = String::new();
        for (port, entry) in self.filtered_entries() {
            let mut obj = serde_json::Map::new();
            if show_timestamp {
                obj.insert("time".into(), serde_json::Value::String(entry.timestamp_label.clone()));
            }
            if show_port {
                obj.insert("port".into(), serde_json::Value::String(port.clone()));
            }
            obj.insert("direction".into(), serde_json::Value::String(match entry.direction {
                Direction::Rx => "RX".into(),
                Direction::Tx => "TX".into(),
                Direction::Internal => "INTERNAL".into(),
            }));
            if show_hex {
                obj.insert("hex".into(), serde_json::Value::String(entry.hex_text.clone()));
            } else {
                obj.insert("text".into(), serde_json::Value::String(entry.raw_text.clone()));
            }
            out.push_str(&serde_json::to_string(&serde_json::Value::Object(obj)).unwrap_or_else(|_| "{}".to_owned()));
            out.push('\n');
        }
        out
    }
```

- [ ] **Step 3: 更新 clear() 方法重置新字段**

在 `clear()` 方法（第 150-161 行）末尾，`self.auto_scroll = true;` 之前插入：

```rust
        self.show_timestamp = true;
        self.show_port = true;
```

- [ ] **Step 4: 编译检查 + 运行测试**

```powershell
cargo check -p panels 2>&1
cargo test -p panels 2>&1
```

预期：编译通过，所有测试通过。

- [ ] **Step 5: Commit**

```bash
git add crates/panels/src/terminal.rs
git commit -m "feat(terminal): update export methods and clear() for show options"
```

---

### Task 6: 运行完整测试套件验证

- [ ] **Step 1: 运行全部测试**

```powershell
cargo test --all-targets 2>&1
```

预期：所有测试通过。

- [ ] **Step 2: 运行 clippy**

```powershell
cargo clippy --all-targets 2>&1
```

预期：无新增 warning。

- [ ] **Step 3: Commit (如有 clippy 修复)**

```bash
git add -A
git commit -m "chore: clippy fixes for terminal show options"
```

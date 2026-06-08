# v0.1-preview 发布前检查清单

## 构建检查

```bash
cargo fmt
cargo test
cargo clippy
cargo build --release
```

## 主流程检查

### 串口

- [ ] 串口列表能识别插入和拔出。
- [ ] 打开串口后状态栏显示正确。
- [ ] 关闭串口后状态栏恢复。
- [ ] 顶部快捷串口控件和设备页表现一致。
- [ ] 发送区在串口未打开时仍可编辑，打开后可发送。

### 接收区

- [ ] 大量 RX/TX 数据下滚动仍流畅。
- [ ] “暂停/追踪底部”按钮行为正确。
- [ ] 点击行、右键复制、双击详情正常。
- [ ] 详情窗口内容过长时可滚动。
- [ ] 多端口全局视图按事件顺序显示，不按 COM 分组。

### 日志

- [ ] 长 source 不会压到 message。
- [ ] 自动滚动和暂停滚动正常。
- [ ] 回放时日志能重建。

### 动态面板

- [ ] 插件创建的图表、表单、姿态面板正常。
- [ ] 动态标签可以拖动排序。
- [ ] 弹出独立窗口后点 X 是回到标签栏，不是删除面板。
- [ ] 独立窗口没有黑色未绘制区域。

## 插件检查

### hello 模板

- [ ] 启用后输出日志。
- [ ] 创建表单。
- [ ] 修改表单后收到 `ui.form.changed`。
- [ ] 停用后移除面板。

### serial-chart 模板

- [ ] 监听 `transport.serial.default.rx`。
- [ ] 解析数据后发布 `protocol.template.sample`。
- [ ] 图表显示数值字段。
- [ ] 停用后移除图表和表单。

### Replay Analyzer

- [ ] RawSerial 录制后，选择 ReparseRaw 能通过 `replay.lua` 重建图表。
- [ ] replay.lua 不能访问 `ctx.serial`、`ctx.timer`、`ctx.ui`、`ctx.bus.publish`。
- [ ] analyzer 输出事件带 `origin = replay_derived` 和 `recordable = false`。

## 录制/回放检查

### 录制模式

- [ ] 录制中 RecordMode 下拉禁用。
- [ ] RawSerial 只录 `transport.serial.*`。
- [ ] StandardReplay 录 `transport.serial.* + protocol.* + ui.panel.create`。
- [ ] FullDebug 录 live event，但排除 replay/replay_derived/recordable=false。

### 回放策略

- [ ] AutoPreferRecorded 有 `protocol.*` 时走精确回放。
- [ ] ExactRecorded 不运行 analyzer。
- [ ] ReparseRaw 忽略录制里的 `protocol.*`，使用 analyzer cache。
- [ ] 拖进度条时不重新运行 analyzer。
- [ ] 步进、步退、拖动进度条都能更新进度条和面板。
- [ ] 图表没有重复曲线、锯齿错乱或旧数据混入。

## 文档检查

- [ ] `docs/README.md` 可作为入口。
- [ ] `PLUGIN_DEVELOPMENT.md` 能让用户理解插件结构。
- [ ] `plugin-manifest.md` 说明 `live/replay`。
- [ ] `lua-plugin-api.md` 列出实时 API。
- [ ] `replay-analyzer.md` 说明回放解析器。
- [ ] `recording-replay.md` 说明录制模式和回放策略。
- [ ] 示例插件有 README。

## 已知限制建议写进发布说明

- 拖动系统标题栏时，UI 刷新可能暂停。这是原生窗口事件循环行为。
- Replay Analyzer 当前同步运行，大文件重新解析可能短暂停顿。
- `ctx.storage` 当前不要承诺为稳定持久化配置数据库。
- Lua API 仍处于 `0.1-preview`，未来可能有兼容性调整。

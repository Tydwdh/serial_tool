# Refactor Checklist

## Baseline (2026-06-10)

- cargo fmt --all --check: PASS (2 historical warnings in hardware-workbench-app)
- cargo check --workspace: PASS
- cargo test --workspace: PASS (63 tests)

## Manual Verification Checklist

### 启动/布局
- [ ] 应用启动无 panic
- [ ] 中文字体正常
- [ ] 暗色主题正常
- [ ] 保存布局后重启能恢复

### 串口
- [ ] 无串口时 UI 正常
- [ ] 插入串口后自动发现
- [ ] 打开串口成功
- [ ] 已打开时显示"重连"
- [ ] 关闭串口成功
- [ ] 串口拔出后状态提示
- [ ] 文本发送成功
- [ ] HEX 发送成功
- [ ] HEX 错误提示正常

### 录制
- [ ] 开始录制
- [ ] 停止录制
- [ ] 文件路径正确
- [ ] 录制模式切换正常

### 回放
- [ ] 打开 replay 文件
- [ ] seek 正常
- [ ] step backward 正常
- [ ] analyzer 正常运行
- [ ] analyzer 错误能显示

### 插件
- [ ] 插件列表正常
- [ ] 启用插件
- [ ] 禁用插件
- [ ] 动态面板创建
- [ ] 禁用插件后动态面板清理

### Lua serial
- [ ] ctx.serial.list 返回端口
- [ ] ctx.serial.open 大小写兼容
- [ ] ctx.serial.write_line_and_expect 不误吃旧 ok
- [ ] timeout 能返回错误

## Refactor Stages

### Stage 1: app/src/main.rs split
- [ ] 1.1 Create module files
- [ ] 1.2 main.rs → entry point only
- [ ] 1.3 bootstrap.rs → constants, fonts, theme
- [ ] 1.4 state.rs → status/enum/struct types
- [ ] 1.5 app.rs → WorkbenchApp + eframe::App

### Stage 2: UI function extraction
- [ ] 2.1 ui/mod.rs
- [ ] 2.2 Extract panels to ui/*.rs

### Stage 3: tick/commands/config/replay_task
- [ ] 3.1 config.rs
- [ ] 3.2 commands.rs
- [ ] 3.3 replay_task.rs
- [ ] 3.4 tick.rs

### Stage 4: Group WorkbenchApp state
- [ ] 4.2 SerialUiState
- [ ] 4.3 SendUiState
- [ ] 4.4 StatusState

### Stage 5-6: Extension + Lua Host (separate PRs)

### Stage 7: Final cleanup

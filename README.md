# 硬件调试工作台 (Hardware Workbench)

基于 Rust + egui 的跨平台串口调试工具，支持实时数据可视化、Lua 插件扩展和录制回放。

## 功能

- **串口通信** — 完整的串口配置（波特率、数据位、停止位、校验位），支持 DTR/RTS 信号控制
- **实时终端** — 文本/HEX 双模式显示，支持时间戳、多行渲染、搜索过滤、CSV/JSONL 导出
- **数据可视化** — 内置折线图、3D 姿态视图，支持自动缩放和手动 Y 轴
- **发送器** — 文本/HEX 发送，换行符配置，发送历史，周期发送（微秒级精度）
- **Lua 插件** — 自定义协议解析、数据转换、UI 面板，热加载
- **录制回放** — JSONL 格式录制串口数据，支持快进/快退/步进/分析器
- **可配置快捷键** — VSCode 风格默认绑定，设置面板可视化编辑
- **工作区持久化** — 窗口布局、串口配置、插件状态自动保存

## 快捷键

| 快捷键 | 功能 |
|--------|------|
| `Ctrl+R` | 刷新串口列表 |
| `Ctrl+Shift+O` | 打开/关闭选中串口 |
| `Ctrl+B` | 切换左侧活动栏 |
| `` Ctrl+` `` | 切换底部面板 |
| `Alt+Ctrl+B` | 切换右侧边栏 |
| `Ctrl+1~4` | 切换活动面板（设备/回放/插件/设置） |
| `Ctrl+Enter` | 发送 |

快捷键可在设置面板中自由配置。

## 构建运行

### 环境要求

- Rust 1.85+ (edition 2024)
- Windows / macOS / Linux

### 编译

```bash
# Debug 构建
cargo build

# Release 构建（优化体积）
cargo build --release
```

### 运行

```bash
cargo run
```

### 测试

```bash
cargo test        # 全部 235 个测试
cargo clippy      # 零警告
```

## 项目结构

```
crates/
├── app/           # 应用层：UI、配置、快捷键、事件循环
│   └── src/ui/    # 活动栏、Dock、设置面板、设备面板等
├── core/          # 核心类型：Event、Payload、LogLevel
├── databus/       # 发布/订阅事件总线
├── transport/     # 串口抽象层：打开/关闭/读写/信号控制
├── panels/        # 面板实现：终端、日志、图表、发送器、回放
│   └── src/dynamic/  # 动态面板（Lua 插件创建）
├── lua_host/      # Lua 运行时：沙箱、API 绑定、协议编解码
├── extension/     # 插件管理：发现、加载、权限控制
├── recorder/      # JSONL 录制与回放引擎
└── testing/       # 测试辅助
```

## 插件开发

参见 [docs/](docs/) 目录：

- [PLUGIN_DEVELOPMENT.md](docs/PLUGIN_DEVELOPMENT.md) — 插件开发总览
- [plugin-manifest.md](docs/plugin-manifest.md) — plugin.json 字段说明
- [lua-plugin-api.md](docs/lua-plugin-api.md) — Lua API 参考

## License

MIT

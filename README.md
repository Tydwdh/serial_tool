# 硬件调试工作台 (Hardware Workbench)

基于 Rust + egui 的跨平台串口调试工具，面向硬件调试、串口数据观察、录制回放和 Lua 插件扩展。

## 主要功能

- 串口通信：波特率、数据位、停止位、校验位、DTR/RTS 控制。
- 实时终端：文本/HEX 显示、时间戳、多行渲染、搜索过滤、CSV/JSONL 导出。
- 数据可视化：折线图、3D 姿态视图、自动缩放和手动 Y 轴。
- 发送器：文本/HEX 发送、换行符配置、发送历史、周期发送。
- Lua 插件：协议解析、数据转换、动态面板、工具栏动作和回放解析器。
- 录制回放：JSONL 录制、快进/快退/步进、回放阶段重新解析。
- 工作区持久化：窗口布局、串口配置、插件状态和快捷键配置自动保存。

## 发布形态

正式发布建议提供两种包：

- Portable zip：解压后直接运行，适合临时调试和免安装使用。
- Windows installer：安装到当前用户目录，卸载时删除应用文件、日志、主配置和插件配置，避免残留。

当前发布包默认不预装个人脚本或测试脚本。应用会创建空的 `plugins\` 目录，用户可以从独立脚本仓库下载插件后放入 `plugins\<plugin-id>\`。源码仓库中的模板插件仅用于开发参考，不作为默认启用功能。

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

快捷键可在设置面板中修改。

## 构建

环境要求：

- Rust 1.85+
- Windows / macOS / Linux
- Windows 安装器需要 Inno Setup 6

Windows PowerShell 5.1：

```powershell
Set-Location "C:\Users\tyd27\Desktop\tool"
cargo build
cargo run
cargo test --all-targets
```

生成便携包：

```powershell
Set-Location "C:\Users\tyd27\Desktop\tool"
.\package.bat
```

生成 Windows 安装器：

```powershell
Set-Location "C:\Users\tyd27\Desktop\tool"
.\installer\build-installer.ps1
```

## 项目结构

```text
crates/
  app/           应用层：UI、配置、快捷键、事件循环
  core/          核心类型：Event、Payload、LogLevel
  databus/       发布/订阅事件总线
  transport/     串口抽象层
  panels/        终端、日志、图表、发送器、回放等面板
  lua_host/      Lua 运行时、沙箱和 API 绑定
  extension/     插件发现、加载、权限控制
  recorder/      JSONL 录制与回放
  testing/       测试辅助
docs/            插件和发布文档
plugins/         开发期插件模板与本地脚本
installer/       Windows 安装器配置
```

## 插件开发

参见 [docs/README.md](docs/README.md)。

## License

MIT

# 硬件调试工作台 (Hardware Workbench)

基于 Rust + egui 的桌面串口调试工具，面向硬件调试、串口数据观察、录制回放和 Lua 插件扩展。

项目仓库：https://github.com/Tydwdh/serial_tool

## 主要功能

- **串口通信**：可自定义波特率（含 1M/2M/3M 高速档）、数据位、停止位、校验位、DTR/RTS 控制；自动重连（拔插后自动恢复，可一键取消）。
- **网络串口**：通过 WebSocket + JSON-RPC 连接 Nexus Prime 等 Klipper/Moonraker 系服务器（默认 7125 端口），把发送内容作为 gcode 命令执行、响应回传终端，体验与本地串口一致（终端/发送器/录制/回放全部复用）。
- **实时终端**：文本/HEX 显示、时间戳、跨包按行拼接、搜索过滤和日志导出。
- **数据可视化**：折线图、3D 姿态视图、自动缩放和手动 Y 轴。
- **发送器**：文本/HEX 发送、换行符配置、发送历史（搜索/单条删除/清空）、周期发送（间隔校验阻止非法启用）、Ctrl+Enter 发送。
- **Lua 插件**：协议解析、数据转换、动态面板、工具栏动作和回放解析器；插件市场一键安装/卸载。
- **录制回放**：JSONL 录制、快进/快退/步进、速度预设（0.5x/1x/2x/5x/10x）、回放阶段重新解析。
- **命令面板**：Ctrl+K 搜索并执行内置命令或插件命令。
- **工作区持久化**：窗口布局、串口配置、插件状态、快捷键、终端/日志参数自动保存。

## 快捷键

| 快捷键 | 功能 |
|--------|------|
| `Ctrl+R` | 刷新串口列表 |
| `Ctrl+Shift+O` | 打开/关闭选中串口 |
| `` Ctrl+` `` | 切换底部面板 |
| `Alt+Ctrl+B` | 切换右侧边栏 |
| `Ctrl+Enter` | 发送 |
| `Ctrl+K` | 打开命令面板 |

所有快捷键均可在设置面板中自定义。`StartRecording`、`ReconnectPort`、`AddBookmark` 默认未绑定，可手动设置。

应用运行时从可执行文件同级的 `assets\`、`themes\` 和 `plugins\` 目录加载资源；用户配置写入系统配置目录。

## 配置

配置文件位于：`%APPDATA%\HardwareWorkbench\workspace.json`。

可配置项（部分通过设置面板暴露，也可直接编辑 JSON）：

- 串口参数（波特率/数据位/停止位/校验位）、端口别名与分组、网络串口列表（主机 + 端口）
- 等宽字体大小（10–24px）
- 终端合并阈值（0–100ms，同端口同方向间隔 ≤ 此值且不含换行符的连续包合并）
- 终端/日志保留条数（500–50000）
- 快捷键映射
- 自动重连开关

配置采用原子写入（先写临时文件再 rename），崩溃时不会留下半写文件，旧文件备份为 `.backup`。

## 构建

环境要求：

- Rust 1.92+
- Windows / macOS / Linux
- Windows 安装器需要 Inno Setup 6

```powershell
git clone https://github.com/Tydwdh/serial_tool.git
Set-Location serial_tool
cargo build -p hardware-workbench-app
```

生成 Windows 便携包：

```powershell
.\package.bat
```

打包结果在 `dist\hardware-workbench-app\` 与 `dist\hardware-workbench-app.zip`。

## 发布形态

正式发布提供两种包：

- **Portable zip**：解压后直接运行，适合临时调试和免安装使用。
- **Windows installer**：安装 Inno Setup 6 后执行 `installer\build-installer.ps1`；安装器使用同一 Rust 便携输出。

发布包默认不预装个人脚本或测试脚本。应用会创建空的 `plugins\` 目录，用户可从插件市场安装或手动放入 `plugins\<plugin-id>\`。源码仓库中的模板插件仅用于开发参考，不作为默认启用功能。

## 项目结构

```text
crates/
  application/   UI 无关的应用核心：Workbench + AppCommand/Query/Event（不依赖 egui）
  app/           egui 应用壳：eframe 生命周期、dock、快捷键、对话框、主题
  core/          核心类型：Event、Payload、LogLevel
  databus/       发布/订阅事件总线（唯一 EventBus）
  transport/     串口抽象层（本地 + 网络串口）
  panels/        egui 呈现层面板（终端/日志/图表/回放等，纯 View）
  lua_host/      Lua 运行时、沙箱和 API 绑定
  extension/     插件发现、加载、权限控制
  recorder/      JSONL 录制与回放
  testing/       测试辅助
  updater/       自动更新
docs/            插件和发布文档
plugins/         开发期插件模板与本地脚本
plugin-marketplace/ 插件市场索引、发布脚本与版本化安装包
installer/       Windows 安装器配置
```

架构详见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) 与 [docs/architecture-refactor-baseline.md](docs/architecture-refactor-baseline.md)。

## 插件开发

参见 [docs/README.md](docs/README.md)。

## 发布与变更

- [变更记录](CHANGELOG.md)
- [v1.0.0 发布说明](docs/releases/v1.0.0.md)
- [发布流程](docs/RELEASE.md)
- [安装器说明](docs/INSTALLER.md)

## License

[MIT](LICENSE)。第三方组件与字体许可见
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) 和
[assets/FONT_LICENSES.md](assets/FONT_LICENSES.md)。

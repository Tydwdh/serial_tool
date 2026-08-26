# Web 端能力状态

浏览器端和桌面端使用同一套 egui/egui_tiles 外壳、Dock 布局、核心 Terminal/Chart
面板、Application 命令边界和 DataBus；平台差异只在 composition root 和 capability
实现中。Web 端由
`crates/app/src/web.rs` 组合，并由 `tool_application::web::WebApplication` 收口
命令与异步任务。

Settings、Recorder、Replay、Plugins、Marketplace 和 Updater 的 Web 内容由浏览器
capability 驱动，但已复用共享 Dock、串口、录制、回放策略、数据设置和快捷键组件。
当前仍不是逐像素副本：浏览器的文件、插件运行时、更新和权限模型必须使用平台等价
语义，不能伪装成桌面端能力。

## 已包含

- `eframe::WebRunner` 浏览器启动入口：Trunk 负责加载 wasm，`wasm_bindgen(start)` 自动创建 WebRunner。
- egui UI 外壳、内嵌中文/等宽字体和主题切换。
- `DataBus`、`TerminalPanel`、`ChartPanel`。
- 基础设置、主题和串口参数持久化（`SettingsStore`，当前使用 localStorage 适配器）。
- 内置主题和桌面端同格式的自定义 JSON 主题导入；自定义主题文本会随浏览器设置恢复。
- `PortId` / `PortDescriptor` / `TransportCapabilities` 平台能力模型。
- Web Serial 的异步 `getPorts`、`requestPort`、`open`、RX/TX、DTR/RTS 和拔出断开事件。
- 网络串口使用浏览器 WebSocket 连接 Nexus/Moonraker 的 JSON-RPC G-code 接口，
  支持保存、连接、收发和断线重连。
- Serial 面板支持 TEXT/HEX 发送。
- Native 与 Web 共用 `tool-panels::SerialPanel` 的串口参数、能力、端口动作、分组/别名编辑和收发 DTO；网络端口与录制由各自 composition root 提供。
- Native 与 Web 共用录制卡片和回放策略选择器；路径/下载、浏览器文件选择器和
  Replay Analyzer 由各自运行时实现，但策略和派生事件模型保持一致。
- Web 设置包含与桌面端相同的端口 profile、数据限制和快捷键配置；Ctrl+K 命令面板支持内置命令及已启用 Web 插件命令。
- Web 快捷键设置也会列出已启用 Lua 插件贡献的命令；插件命令使用与 Native 相同的
  `plugin_id:command_id` 持久化键，并通过统一 Lua 引擎分发。
- Native/Web 终端/日志导出使用带快照边界的增量游标：每帧只扫描有限条目并格式化，再交给后台写入或生成 Blob 下载；导出自身也进入
  `TaskId` 的 Pending/Running/Completed/Failed/Cancelled 生命周期，避免 50k 行在点击瞬间构造临时可见列表。
- 回放读取、插件文件导入、市场索引/插件下载、更新检查和插件文件选择都通过
  `WebApplication` 的 `TaskId`/`WebAppEvent` 生命周期完成；任务终态快照会限量保留。
- 录制停止后的 JSONL 序列化也按批次让出 UI 帧，避免大录制在点击停止时同步冻结页面。
- 事件驱动的 egui repaint：后台 Promise、RX、断开和任务状态事件都会主动唤醒 UI，空闲时不持续重绘。
- Web 性能诊断：浏览器控制台每 5 秒输出 frame p50/p95/p99、RX/TX 吞吐、事件速率、
  DataBus publish 平均耗时、subscriber backlog/drop、Recorder backlog，以及
  Terminal/Log/Chart 最近一次渲染耗时；采样窗口有界，不会因诊断本身造成内存增长。

另外已经接入浏览器等价能力：

- Recorder：DataBus lossless 订阅、积压软/硬阈值、已录制事件数/字节数硬上限、JSONL Blob 下载；达到任一硬阈值会停止并标记不完整，避免无限内存增长。
- Replay：浏览器文件选择器读取 JSONL，播放/暂停/停止/单步/倍速/循环/拖动定位和书签；
  Lua 插件可通过 `replay` manifest 和 `on_replay_*` 回调提供与 Native
  Analyzer 对应的原始事件重解析能力；输入按 256 条批次确认，避免大文件整段
  序列化阻塞 UI。
- Plugin：Native/Web 共用 `plugin.json` + `main.lua`、manifest/permissions、DataBus、串口收发、日志、动态面板、设置和持久化 storage；Web 使用纯 Rust Lua VM。
- 动态表单的文件字段在 Web 中通过用户文件选择器异步读取文本，插件收到的文件事件同时包含文件名和文本内容。
- 插件列表、市场筛选和启用/禁用/重启/卸载交互直接复用 `tool-panels::PluginsPanel`；Web
  runtime 只向共享 UI 提供 `PluginView` / `MarketplaceStatusView`，Lua VM 和宿主句柄不会泄漏到面板。
- Marketplace：读取远程 registry，安装统一 Lua manifest + main.lua；不再维护 Web 专用插件包。
- 插件 UI contribution：Web 纯 Rust Lua VM 支持与 Native 相同的 `send.toolbar`、`status_bar.*`、`top_bar.*` slot；运行时进度/状态通过 `ctx.ui.set_contribution_value` 更新。G-code Sender 的文件发送、单条发送、暂停/取消和确认进度已走这条共享通道。
- Updater：检查远程版本信息并打开发布页。浏览器不能像桌面端一样原地替换正在运行的 WASM，因此这里是“检查/引导下载”能力，不是假装具备原生自更新。

Web 应用的启动和刷新不会主动触发浏览器权限请求。用户必须显式点击“添加设备”，之后才会进入 Web Serial 的 `requestPort()` 流程。

## Composition root

| 平台 | 入口 | 组合内容 |
| --- | --- | --- |
| Native | `main` → `eframe::run_native` | 现有 Native `WorkbenchApp` 及完整服务 |
| Web | 导出 `start(canvas_id)` → `eframe::WebRunner` | 共享 Dock、DataBus、Terminal、Log、Chart、设置、Recorder、Replay、Plugin、WebApplication、Web Serial |

本地开发和构建：

```text
cargo check --target wasm32-unknown-unknown -p hardware-workbench-app
cd web
trunk serve --release
```

`trunk serve --release` 默认打开 `http://127.0.0.1:8080/`，用于本地验证接近生产的 WASM 性能。
如只需要快速迭代编译，也可以使用 `trunk serve`，但它是 debug 构建，交互性能会明显下降。
生产构建使用：

```text
trunk build --release --public-url /serial_tool/
```

GitHub Pages 工作流会把构建结果发布到 `/serial_tool/`，HTML 中的
`data-trunk-public-url` 会让资源和 wasm-bindgen 加载路径跟随这个前缀。

## 当前 capability 状态

1. Transport 已抽象成 `PortId` / `PortDescriptor` / `TransportCapabilities`，Native 与 Web 各自实现异步 backend。
2. SettingsStore 和 FileService 已位于 `tool-platform` capability 层；Native 配置/导出走 worker-backed service，Web 设置使用浏览器存储适配器。
3. Web Serial 已接入 `WebApplication`，完成 requestPort、可配置串口参数、RX/TX、DTR/RTS 和拔出断开事件；
   WebSocket 网络串口复用同一任务和 DataBus 事件边界。

## 浏览器硬件验收

WASM 编译和 Trunk 打包可以在 CI 验证；真实浏览器硬件矩阵需要在 Chrome/Edge
中手工执行，当前 CI 不连接 CH340、CP2102 或 STM32。至少要覆盖添加设备、连接、
文本/HEX 收发、无效 UTF-8、高速 RX、DTR/RTS、拔出、重插以及刷新/重新加载后的重新授权。

## 明确的非等价项

1. `mlua` 原生运行时不能编译到 `wasm32-unknown-unknown`；浏览器改用纯 Rust Lua VM，
  执行与 Native 相同的 Lua 源码。仍需逐项验证第三方插件使用的 capability 是否有 Web
  等价实现。
2. 浏览器没有原生文件系统路径和原地二进制更新语义。录制/导出使用下载，回放使用
   用户选择的文件；更新使用版本检查和发布页跳转。
3. Web Serial 受浏览器、HTTPS/localhost、用户手势和设备驱动限制。网络串口受
   WebSocket mixed-content/CORS、服务器监听地址和 `ws`/`wss` 配置限制；系统
   COM 端口全量枚举等 Native 语义不适用于浏览器。

因此，Web 不需要维护第二套 HTML/CSS UI。插件也继续使用同一份
`plugin.json + main.lua`；Web 运行时只替换 Lua VM 和 capability host，不改变插件格式。
达到生产级对标前仍必须完成：Chrome/Edge 真实硬件矩阵、长时间高速 RX/导出压力测试，
以及任意第三方 Lua 插件在各项 capability 上的逐项兼容性审计。浏览器不具备的能力仍会
明确返回“不支持”，而不是偷偷提供一套不同的插件 API。

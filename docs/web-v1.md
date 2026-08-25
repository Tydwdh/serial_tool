# Web V1 边界

Web V1 先证明 Hardware Workbench 的 egui UI 和平台无关数据路径可以在浏览器运行。浏览器端不复用 Native `Workbench` composition root，而是由 `crates/app/src/web.rs` 组合，并由 `tool_application::web::WebApplication` 收口命令与异步任务。

## 已包含

- `eframe::WebRunner` 浏览器启动入口：Trunk 负责加载 wasm，`wasm_bindgen(start)` 自动创建 WebRunner。
- egui UI 外壳、内嵌中文/等宽字体和主题切换。
- `DataBus`、`TerminalPanel`、`ChartPanel`。
- 基础设置、主题和串口参数持久化（`SettingsStore`，当前使用 localStorage 适配器）。
- `PortId` / `PortDescriptor` / `TransportCapabilities` 平台能力模型。
- Web Serial 的异步 `getPorts`、`requestPort`、`open`、RX/TX、DTR/RTS 和拔出断开事件。
- Serial 面板支持 TEXT/HEX 发送。
- 事件驱动的 egui repaint：后台 Promise、RX、断开和任务状态事件都会主动唤醒 UI，空闲时不持续重绘。

## 第一阶段明确关闭

- Updater
- Marketplace
- Lua Plugin
- Recorder
- Replay

Web V1 的启动和刷新不会主动触发浏览器权限请求。用户必须显式点击“添加设备”，之后才会进入 Web Serial 的 `requestPort()` 流程。

## Composition root

| 平台 | 入口 | 组合内容 |
| --- | --- | --- |
| Native | `main` → `eframe::run_native` | 现有 Native `WorkbenchApp` 及完整服务 |
| Web | 导出 `start(canvas_id)` → `eframe::WebRunner` | DataBus、Terminal、Chart、设置/主题、WebApplication、Web Serial |

本地开发和构建：

```text
cargo check --target wasm32-unknown-unknown -p hardware-workbench-app
cd web
trunk serve
```

`trunk serve` 默认打开 `http://127.0.0.1:8080/`。生产构建使用：

```text
trunk build --release --public-url /serial_tool/
```

GitHub Pages 工作流会把构建结果发布到 `/serial_tool/`，HTML 中的
`data-trunk-public-url` 会让资源和 wasm-bindgen 加载路径跟随这个前缀。

## 当前 capability 状态

1. Transport 已抽象成 `PortId` / `PortDescriptor` / `TransportCapabilities`，Native 与 Web 各自实现异步 backend。
2. SettingsStore 和 FileService 已位于 `tool-platform` capability 层；Native 配置/导出走 worker-backed service，Web 设置使用浏览器存储适配器。
3. Web Serial 已接入 `WebApplication`，完成 requestPort、可配置串口参数、RX/TX、DTR/RTS 和拔出断开事件。

## 浏览器硬件验收

WASM 编译和 Trunk 打包可以在 CI 验证；真实浏览器硬件矩阵需要在 Chrome/Edge
中手工执行，当前 CI 不连接 CH340、CP2102 或 STM32。至少要覆盖添加设备、连接、
文本/HEX 收发、无效 UTF-8、高速 RX、DTR/RTS、拔出、重插以及刷新/重新加载后的重新授权。

后续批次：

1. Recorder/Replay：Native 保持文件系统，Web 再接 OPFS/浏览器文件句柄。
2. Plugin API/manifest/permissions 保持平台无关；Web runtime 另行确定，不能把 `mlua`/Emscripten 带进当前 `wasm32-unknown-unknown` 目标。
3. Marketplace/Updater 最后处理。

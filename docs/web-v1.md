# Web V1 边界

Web V1 先证明 Hardware Workbench 的 egui UI 和平台无关数据路径可以在浏览器运行。浏览器端不复用 Native `Workbench` composition root，而是由 `crates/app/src/web.rs` 组合，并由 `tool_application::web::WebApplication` 收口命令与异步任务。

## 已包含

- `eframe::WebRunner` 浏览器启动入口：导出 `start(canvas_id)`，由 HTML/JavaScript 在 canvas 就绪后调用。
- egui UI 外壳、内嵌中文/等宽字体和主题切换。
- `DataBus`、`TerminalPanel`、`ChartPanel`。
- 基础设置和主题持久化（`SettingsStore`，当前使用 localStorage 适配器）。
- `PortId` / `PortDescriptor` / `TransportCapabilities` 平台能力模型。
- Web Serial 的异步 `getPorts`、`requestPort`、`open`、RX/TX、DTR/RTS 和拔出断开事件。

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

目标检查：

```text
cargo check --target wasm32-unknown-unknown
```

浏览器宿主需要提供一个 canvas，并在 wasm-bindgen 生成的 JS 模块加载后调用：

```js
await wasm_bindgen.start("workbench-canvas");
```

## 当前 capability 状态

1. Transport 已抽象成 `PortId` / `PortDescriptor` / `TransportCapabilities`，Native 与 Web 各自实现异步 backend。
2. SettingsStore 和 FileService 已位于 `tool-platform` capability 层；Native 配置/导出走 worker-backed service，Web 设置使用浏览器存储适配器。
3. Web Serial 已接入 `WebApplication`，完成 requestPort、115200 8N1、RX/TX、DTR/RTS 和拔出断开事件。

后续批次：

1. Recorder/Replay：Native 保持文件系统，Web 再接 OPFS/浏览器文件句柄。
2. Plugin API/manifest/permissions 保持平台无关；Web runtime 另行确定，不能把 `mlua`/Emscripten 带进当前 `wasm32-unknown-unknown` 目标。
3. Marketplace/Updater 最后处理。

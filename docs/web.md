# 在线模式（WASM）

浏览器端与桌面端共用同一套 egui / egui_tiles 外壳、Dock 布局、Terminal / Log / Chart
面板、Application 命令边界和 DataBus。平台差异只体现在 composition root 与各
capability 的实现上，Web 端不维护第二套 HTML/CSS UI。

- Web 组合根：`crates/app/src/web/`（`mod.rs` 为入口，`serial.rs` / `plugins.rs` /
  `replay.rs` 按域拆分，依据 `crates/app/src/web/mod.rs:18-21`）。
- 命令与异步任务收口：`tool_application::web::WebApplication`
  （`crates/application/src/web.rs`）。
- 平台能力：`crates/platform/src/web_serial.rs`、`web_network.rs`、`web_fetch.rs`、
  `storage.rs`，均在 `#[cfg(target_arch = "wasm32")]` 下编译
  （`crates/platform/src/lib.rs:15-22`）。

浏览器入口是 `#[wasm_bindgen(start)] bootstrap()`，它调用 `start(canvas_id)`
（`crates/app/src/web/mod.rs:3648,3673`）；HTML 宿主只负责提供 canvas，不掺入
wasm-bindgen 生成模块名。

## 在线模式能做什么

| 能力 | 实现位置 | 说明 |
| --- | --- | --- |
| Web Serial 串口 | `crates/platform/src/web_serial.rs` | 异步 `getPorts` / `requestPort` / `open` / RX / TX / DTR / RTS，含拔出断开事件；每个打开端口一个 FIFO 写者 actor |
| WebSocket 网络串口 | `crates/platform/src/web_network.rs` | 连接 Nexus / Moonraker 的 JSON-RPC G-code 接口，支持保存、连接、收发与断线重连 |
| 完整工作台 UI | `crates/app/src/web/mod.rs` | 共享 Dock、设置、快捷键（含 Ctrl+K 命令面板）、Terminal / Log / Chart |
| 内存录制 | `crates/application/src/web.rs:291-295` | DataBus lossless 订阅 + 内存缓冲，停止后序列化为 JSONL 并触发 Blob 下载 |
| 回放 | `crates/application/src/web.rs` | 浏览器文件选择器读取 JSONL，播放 / 暂停 / 停止 / 单步 / 倍速 / 循环 / 定位 / 书签 |
| 浏览器插件 VM | `crates/plugin_runtime/src/web_lua.rs` | 纯 Rust omniLua，执行与 Native 相同的 Lua 源码 |
| 市场安装 | `crates/application/src/web.rs:755` | 读取远程 registry，安装统一 Lua manifest + `main.lua` |
| 检查更新 | `crates/application/src/web.rs:831` | 拉取远程版本信息并在状态栏 / 设置页提示，打开下载页 |

- 设置、主题与串口参数经 `tool-platform` 存储适配器持久化到 localStorage
  （`crates/platform/src/storage.rs:302`）。
- 网络端口与 Web Serial 走同一套 `PortId` / `PortDescriptor` / `TransportCapabilities`
  模型与 `TaskId` / `WebAppEvent` 生命周期。
- Serial 面板支持 TEXT / HEX 发送，HEX 判定与 Native 同源：`WebApplication::validate_hex`
  调用 `tool_core::{parse_hex, parse_hex_strict}`（`crates/application/src/web.rs` 注释段）。
- 导出（终端 / 日志 / 录制）带快照边界的增量游标，逐帧扫描有限条目后生成 Blob 下载，
  不进 `TaskId` 之外的临时可见列表。

## 能力矩阵

以 `crates/application/src/capability.rs` 的 `AppCapabilities` 为唯一来源
（`native()` 见 `capability.rs:33`，`web()` 见 `capability.rs:47`）。

| Capability | Native | Web | Web 语义 |
| --- | --- | --- | --- |
| Serial | 是 | 是 | Web Serial |
| RequestPort | 否 | 是 | 仅用户手势触发 `navigator.serial.requestPort()` |
| FileSystem | 是 | 否 | 浏览器无原生路径；导出走 Blob 下载，导入走文件选择器 |
| Recorder | 是 | 是 | 内存 lossless 录制，到硬上限即停止标记不完整 |
| Replay | 是 | 是 | 浏览器文件选择器读取 JSONL |
| Plugins | 是 | 是 | 纯 Rust omniLua VM，同一份 `plugin.json` + `main.lua` |
| Marketplace | 是 | 是 | 远程 registry 安装 Lua 插件 |
| Updater | 是 | 是 | 仅检查版本 + 打开发布页，不做原地替换 |
| NetworkSerial | 是 | 是 | 浏览器 WebSocket 后端 |

## 非等价项

1. **Lua 运行时**：`mlua` 原生运行时不能编译到 `wasm32-unknown-unknown`，浏览器改用纯
   Rust omniLua（`crates/plugin_runtime/src/web_lua.rs:1-18`）。两端共用
   `PluginValue` / `PluginHostApi` 协议，运行同一份 Lua 源码；沙箱在脚本执行前移除
   `dofile`、`loadfile`、`os.execute` 和整个 `io` 表（`web_lua.rs:99-111`），插件在浏览器
   中无法触及文件系统或派生子进程。第三方插件使用的 capability 是否都有 Web 等价实现
   仍需逐项审计（见 [plugins/lua-api.md](plugins/lua-api.md)）。
2. **文件系统**：浏览器没有原生路径与原地二进制更新语义。录制 / 导出使用下载，回放使用
   用户选择的文件，更新仅做版本检查与发布页跳转。
3. **更新**：桌面端由 `crates/app/src/runtime/update.rs` 的本地更新线程完成下载与重启
   （该模块仅在非 wasm 下编译，`crates/app/src/main.rs:25-26`）；Web 端只检查版本
   （`crates/application/src/web.rs:831`），不做原地替换。
4. **Web Serial 授权**：`request_port` 只能在用户手势中调用，后台刷新只映射到浏览器已
   授权的 `navigator.serial.getPorts()`（`crates/platform/src/web_serial.rs:3-5`）。启动
   与刷新不会主动请求权限，用户必须显式点击「添加设备」。受浏览器、HTTPS / localhost、
   用户手势与设备驱动限制。
5. **网络串口**：受 WebSocket mixed-content / CORS、服务器监听地址与 `ws` / `wss` 配置
   限制；页面为 HTTPS 时自动使用 `wss`，否则用 `ws`（`crates/platform/src/web_network.rs`
   的 `websocket_url`）。系统 COM 端口全量枚举等 Native 语义不适用于浏览器。

## 构建与部署

WASM 目标的检查命令（在仓库根执行）：

```text
cargo check --target wasm32-unknown-unknown --workspace --exclude tool-extension --exclude tool-lua-host --exclude tool-marketplace --exclude tool-transport --exclude tool-updater
```

本地开发：

```text
cd web
trunk serve --release
```

`trunk serve --release` 默认打开 `http://127.0.0.1:8080/`（`web/Trunk.toml` 的 `[serve]`），
用于接近生产的 WASM 性能验证。只需快速迭代可用 `trunk serve`，但它是 debug 构建，交互
性能会明显下降。

生产构建：

```text
trunk build --release --public-url /serial_tool/
```

- `web/index.html` 的 `<base data-trunk-public-url>` 让资源与 wasm-bindgen 加载路径跟随
  前缀（`web/index.html:6`）；canvas 的 id 为 `hardware-workbench`（`web/index.html:16`）。
- Trunk 配置 `dist = "../dist-web"`、`filehash = true`（`web/Trunk.toml`）。
- wasm 编译开启 `web_sys_unstable_apis`（`.cargo/config.toml`）。

GitHub Pages 工作流（`.github/workflows/web.yml`）：

1. 安装 `wasm32-unknown-unknown` 目标并校验 sysroot 中存在该目标。
2. 在 `web/` 下执行 `trunk build --release --public-url /serial_tool/`
   （`web.yml:45`）。
3. 校验产物（`web.yml:47`）：`dist-web/index.html` 非空、含 `hardware-workbench`；
   至少存在一个非空 `.wasm` 与一个非空 `.js`。
4. 对 wasm 目标跑 `cargo clippy`（排除与浏览器无关的 crate）。
5. `push` 到 `main` 时经 `upload-pages-artifact` / `deploy-pages` 发布到 `/serial_tool/`
   （`web.yml:61,78`）。

## 浏览器硬件验收

WASM 编译与 Trunk 打包由 CI 验证；真实浏览器硬件矩阵需在 Chrome / Edge 中手工执行，
CI 不连接 CH340、CP2102 或 STM32。至少覆盖：

1. 点击「添加设备」，完成 `requestPort()` 授权。
2. 连接后 TEXT 与 HEX 发送、接收。
3. 发送无效 UTF-8 数据的容错表现。
4. 高速 RX 下的稳定性与背压。
5. DTR / RTS 信号线切换。
6. 拔出设备后自动重连（含设备改名后仍能识别同一物理设备）。
7. 刷新 / 重新加载页面后重新授权。

网络串口另外覆盖 WebSocket 连接、收发与断线重连，并在 HTTPS 页面下确认使用 `wss`。

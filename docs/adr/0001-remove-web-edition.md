# 删除 Web 版（wasm32 / 在线模式）

Web 版是与 native 并列的第二个组合根，要求共享层长期保留 wasm 分支、双引擎插件协议、独立 CI 与 Pages 发布。维护成本持续落在每一次共享代码变更上，因此决定在 2.0.0 删除整个 Web 版：wasm 目标与 `crates/app/src/web`、`crates/application/src/web.rs`、`tool-platform` 的 web 模块、`crates/plugin_runtime`（omniLua）及其差分测试、`.github/workflows/web.yml`、`web/` 与 `docs/web.md`。原生网络串口（Moonraker WebSocket）不属于 Web 版，保留。

**Consequences**: `v1.3.2` 是最后一个包含 Web 版的版本，tag 保留、不重写历史；GitHub Pages 停用；发布说明需写明在线版终止与桌面版下载。`AppRuntime` 等双组合根抽象随之收口（见 ADR-0002）。

**Considered Options**: 冻结 Web 版但不再维护 —— 否决，因为共享层的 wasm 分支与双引擎协议仍会在每次改动时收费；保留 `web-archive` 分支 —— 否决，`v1.3.2` tag 已足够找回。

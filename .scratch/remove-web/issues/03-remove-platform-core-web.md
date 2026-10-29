# 03: 移除 platform/core 的 web 适配

**What to build:** `tool-platform` 与 `tool-core` 不再有浏览器串口、网络、fetch、存储适配或 wasm 时钟分支；native 存储与串口行为不变。

**Blocked by:** 02.

**Status:** ready-for-agent

- [ ] platform 的 web serial/network/fetch/storage 模块删除，wasm 导出与 cfg 删除。
- [ ] core 的 wasm 时钟分支与 js-sys 依赖删除。
- [ ] `cargo test -p tool-platform -p tool-core` 通过。

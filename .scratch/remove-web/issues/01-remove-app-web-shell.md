# 01: 移除 app 浏览器壳

**What to build:** `hardware-workbench-app` 不再包含浏览器入口、浏览器插件宿主或 wasm 专属分支；native 应用构建与测试保持绿色。

**Blocked by:** 00.

**Status:** ready-for-agent

- [ ] app 的 web 模块目录、浏览器插件宿主、web 性能模块删除。
- [ ] `main` 不再声明 web 模块；`workbench_app`/`panel_registry`/`reconnect`/`bootstrap` 的 wasm 分支删除。
- [ ] app 的 wasm 专属依赖（js-sys、wasm-bindgen 系列、web-sys、plugin runtime/api 的 wasm 依赖）删除。
- [ ] `cargo check -p hardware-workbench-app` 与 `cargo test -p hardware-workbench-app` 通过。

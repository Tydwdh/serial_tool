# 02: 移除 application 的 web 组合根

**What to build:** `tool-application` 只暴露 native `Workbench`；浏览器组合根、wasm 专属命令与依赖消失，native 契约测试保持绿色。

**Blocked by:** 01.

**Status:** ready-for-agent

- [ ] application 的 web 模块删除；`WebApplication`/`WebRuntime` 及其导出删除。
- [ ] wasm 专属命令变体与执行分支删除；wasm 依赖删除。
- [ ] `error`/`event`/`perf`/`query`/`task`/`workbench` 的 `cfg(not(wasm32))` 门控删除（改为无条件）。
- [ ] `cargo test -p tool-application` 通过，`AppRuntime` 只剩 `Workbench` 一个实现。

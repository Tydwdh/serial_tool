# 04: 收口 AppRuntime 与能力门控

**What to build:** UI 直接持有 `Workbench`；`AppRuntime` trait、`Capability`/`AppCapabilities` 与面板能力门控删除；DTO 边界保留。

**Blocked by:** 02.

**Status:** ready-for-agent

- [ ] `AppRuntime` trait 删除，`query_*`/`capabilities`/`tick`/`dispatch` 成为 `Workbench` 的固有方法。
- [ ] `Capability`/`AppCapabilities` 删除；`PanelDef::required_capability` 与注册表过滤删除。
- [ ] shared shell 与 app 运行时直接使用 `Workbench`；`CommandEffects` 保留。
- [ ] `cargo test -p tool-application -p tool-panels -p hardware-workbench-app` 通过。

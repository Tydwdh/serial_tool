# 收口到单一 native 组合根

Web 版删除后，`AppRuntime` 只剩 `Workbench` 一个实现，`AppCapabilities` 的 native 矩阵除 `RequestPort` 外全为 `true`，内置面板的 `required_capability` 门控恒真。因此决定把 `AppRuntime` 的读取/派发方法收为 `Workbench` 的固有方法，删除 `Capability`/`AppCapabilities` 与 `PanelDef::required_capability`，并随 Web 版删除 `WebCommandEffects`。

**Consequences**: `tool-panels` 继续只依赖 `tool-application` 的 DTO（`TransportView` 等），架构守卫不变；`CommandEffects` 是否收口不在本决定内，另按单实现 seam 评审。

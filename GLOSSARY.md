# Hardware Workbench

Rust + egui 的串口调试工作台。本文件只定义项目内特有、容易混淆的词；实现细节见 `docs/ARCHITECTURE.md`。

## 运行形态

**Native / 桌面版**:
Windows/Linux 上由 `crates/app` 的 eframe 壳 + `Workbench` 组成的唯一运行形态。
_Avoid_: 本地版、客户端

**Web 版 / 在线模式**:
通过 wasm32 + Trunk 构建、由 GitHub Pages 提供的浏览器运行形态；v1.3.2 是最后一个包含它的版本。
_Avoid_: 网页端、浏览器版（除非明确指同一物）

**组合根 (Composition root)**:
把平台实现、应用核心与 UI 壳装配成一个可运行程序的入口；本仓曾有两个（native / web），现在只有 native。
_Avoid_: 入口、main

## 应用层

**Workbench**:
native 的应用核心与组合根，拥有 transport/recorder/replay/plugins，并实现命令执行所需的副作用。
_Avoid_: 应用、后端

**面板 (Panel)**:
`tool-panels` 中只负责渲染并返回 Action 的 View；不直接依赖领域 crate。
_Avoid_: 控件、组件

**能力门控 (Capability gate)**:
按运行形态声明某项能力是否可用的开关；web 删除后 native 全部可用，门控已移除。
_Avoid_: 权限（插件权限是另一概念）

**命令 (AppCommand) / 任务 (Task)**:
UI 动作先变成 `AppCommand`，由唯一 `execute` 决策，异步工作交给 `TaskManager`，结果经 `AppEvent` 回到 `Workbench`。
_Avoid_: 请求、作业

## 传输与数据

**网络串口 (Network serial)**:
通过 WebSocket + JSON-RPC 连接 Moonraker/Klipper 的端口形态；native 与 web 曾各自实现，native 实现保留。
_Avoid_: Web 串口（Web Serial 是浏览器 API，不是这个）

**Web Serial**:
浏览器提供的串口 API，只属于已删除的 Web 版；不要与网络串口混淆。
_Avoid_: 串口（太泛）

**虚拟串口 (Virtual serial)**:
内存中的端口实现，供插件场景测试与 CI 使用。
_Avoid_: 模拟串口、假串口

**DataBus**:
全仓唯一的事件总线；发布者发布 `Arc<Event>`，订阅模式决定背压策略。
_Avoid_: 消息队列、事件系统

**View / DTO**:
`tool-application` 暴露给 `tool-panels` 的只读快照（如 `TransportView`、`PortView`）；跨界数据只走这些类型。
_Avoid_: 模型、状态

## 插件

**插件 (Plugin)**:
放在 `<数据目录>/plugins/<id>/` 下、由 `plugin.json` 声明的 Lua 扩展；运行在 native `lua_host`（mlua）中。
_Avoid_: 扩展、脚本

**插件市场 (Marketplace)**:
`plugin-marketplace/registry.json` 及其指向的版本化 zip；安装时校验 SHA256。
_Avoid_: 商店、仓库

**模板插件 (Template plugin)**:
`plugins/template.*` 下的开发参考，同时被市场索引引用；保留，不作为默认启用功能。
_Avoid_: 示例（template 与 demo 是两类）

# 插件开发总览

Hardware Workbench 的插件是一组放在 `plugins/<plugin-id>/` 下的文件。主程序扫描每个子目录里的 `plugin.json`，加载入口 Lua 脚本即可启用，**不需要重新编译主程序**（`crates/extension/src/manager.rs:150-165`）。

插件列表在应用的插件页刷新时重建；每个插件的解析错误、不兼容 `api_version`、未授权权限都会作为「诊断」显示在该页面（`crates/extension/src/manager.rs:532`，诊断数据结构 `crates/extension/src/manifest.rs:269-295`）。

## 目录结构

```text
plugins/
  plugin.schema.json            # 编辑器用 JSON Schema（与插件目录同级）
  .lua/                         # LuaLS stub（见下文）
    hardware-workbench.lua
    hw/
      codec.lua
      utils.lua
  template.hello/               # 最小插件模板
    plugin.json
    main.lua
    README.md
  <plugin-id>/                  # 你自己的插件
    plugin.json                 # 必需：插件声明
    main.lua                    # 插件入口
```

- `plugin.json`：插件声明，字段见 [manifest.md](manifest.md)。
- `main.lua`：插件入口，脚本文件名由清单 `main` / `live.main` 指定。

## 运行角色

### 实时插件：main.lua

在正常运行时激活，可以打开/关闭串口、发送数据、监听事件、创建动态面板、运行定时器，把串口 RX 解析成 `protocol.*` 事件：

```text
串口 RX -> main.lua 解析 -> ctx.bus.publish("protocol.xxx", {...}) -> 图表
```

可用能力由清单声明的权限决定：只有声明了对应权限，宿主才会挂载 `ctx.serial`、`ctx.ui`、`ctx.timer` 等 API（`crates/lua_host/src/lib.rs:848-945`）。

## 最小实时插件

以 `template.hello` 为准，面板通过 `contributes.panels` 声明，宿主负责创建与销毁，Lua 侧只监听变化、清理自己的状态。

`plugin.json`：

```json
{
  "$schema": "../plugin.schema.json",
  "id": "yourname.hello",
  "name": "Hello 插件",
  "version": "0.1.0",
  "api_version": "0.1",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["bus", "log", "ui"],
  "contributes": {
    "panels": [
      {
        "id": "yourname.hello.form",
        "title": "Hello 参数",
        "kind": "form",
        "auto_apply": true,
        "fields": [
          { "id": "message", "label": "消息", "kind": "text", "default": "hello" }
        ]
      }
    ]
  }
}
```

`main.lua`：

```lua
ctx.log.info("hello plugin started: " .. ctx.plugin.id)

ctx.bus.on("ui.form.changed", function(event)
  if event.payload.panel_id ~= "yourname.hello.form" then
    return
  end
  ctx.log.info("message changed: " .. tostring(event.payload.values.message))
end)

on_disable(function()
  ctx.log.info("hello plugin stopped")
end)
```

完整示例见 `plugins/template.hello/main.lua`（日志分级、`ctx.session` 持久化、`hw.utils` / `hw.codec`）。各 `contributes` 枚举与校验见 [manifest.md](manifest.md)，`ctx.*` API 见 [lua-api.md](lua-api.md)。

## 开发流程

1. 复制模板目录，例如 `plugins/template.hello` -> `plugins/yourname.hello`。
2. 修改 `plugin.json` 的 `id` / `name` / `version`。
3. 修改 `main.lua`，必要时修改面板 ID。
4. 在应用的插件页**刷新**（`crates/extension/src/manager.rs:143`）或重启应用。
5. 启用插件（`crates/extension/src/manager.rs:308`）。
6. 查看日志与动态面板是否正常。

## 设计建议

- **插件 ID 稳定**：`id` 发布后尽量不要改，建议用命名空间风格，如 `yourname.pid-tuner`、`vendor.imu-viewer`。
- **面板 ID 稳定**：面板 ID 带上插件 ID 前缀，避免跨插件冲突，如 `yourname.my-analyzer.chart`。
- **UI 入口优先用命令**：`contributes.ui[].command` 触发 `ctx.commands.register` 注册的 handler，插件只声明控件与命令，不直接绘制宿主 UI（`crates/extension/src/manifest.rs:321-352`）。
- **退出时清理**：在 `on_disable` 中取消定时器、移除运行时创建的动态面板。

## 编辑器支持（LuaLS stub）

仓库根目录的 `.luarc.json` 让 LuaLS 读取 `plugins/.lua/` 下的宿主 API stub，并把 `ctx`、`on_disable` 声明为全局：

```json
"workspace.library": ["plugins/.lua"],
"diagnostics.globals": ["ctx", "on_disable"]
```

`plugins/.lua/hardware-workbench.lua` 提供 `ctx` 的类型定义；`plugins/.lua/hw/codec.lua` 与 `plugins/.lua/hw/utils.lua` 提供 `require("hw.codec")` / `require("hw.utils")` 的类型定义，避免编辑时误报未定义。

如果 Rust 侧新增或改名了 `ctx.*` API，或新增了可 `require` 的内置模块，请同步更新对应的 stub。

模板 `plugin.json` 顶部带 `"$schema": "../plugin.schema.json"`。保持 `plugins/plugin.schema.json` 位于插件目录的上一级，支持 JSON Schema 的编辑器会提示字段、权限、UI 插槽与常见枚举值。

## 市场与分发

插件发布产物（zip + 清单副本 + 全局索引）由 `plugin-marketplace/` 维护，插件源码仍保留在 `plugins/` 下。发布流程、SHA256 校验与客户端安装模型见 [../../plugin-marketplace/README.md](../../plugin-marketplace/README.md)。

## 相关文档

- [manifest.md](manifest.md)——`plugin.json` 字段、权限、`contributes` 枚举与校验。
- [lua-api.md](lua-api.md)——`ctx.*` 运行时 API 与回调。
- [scenarios.md](scenarios.md)——串口插件场景测试 DSL。
- [spec.generated.md](spec.generated.md)——协议枚举自动生成参考（生成物，勿手改）。
- [../ARCHITECTURE.md](../ARCHITECTURE.md)——插件系统在整体架构中的位置。
- [../known-gaps.md](../known-gaps.md)——已知限制与缺口。

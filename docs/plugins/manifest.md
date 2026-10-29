# plugin.json 字段参考

`plugin.json` 是插件声明文件。主程序通过它识别插件 ID、入口脚本、权限与贡献面板（数据模型 `crates/extension/src/manifest.rs:17-72`，运行时校验 `crates/extension/src/manifest.rs:72-170`）。

模板在文件顶部包含：

```json
"$schema": "../plugin.schema.json"
```

`$schema` 只给编辑器使用，主程序忽略它。保持 `plugins/plugin.schema.json` 位于插件目录上一级，可获得字段提示与基础校验。该 Schema 是编辑器侧表面；运行时以 `manifest.rs` 与 `spec.rs` 为准。

## 顶层字段

| 字段 | 类型 | 必填 | 默认 | 说明 |
|---|---|---:|---|---|
| `id` | string | 是 | — | 插件唯一 ID，非空（`manifest.rs:76`）。发布后保持稳定。 |
| `name` | string | 是 | — | UI 显示名，非空（`manifest.rs:79`）。 |
| `version` | string | 是 | — | 插件自身版本，非空（`manifest.rs:82`）。 |
| `api_version` | string | 否 | `"0.1"` | 依赖的宿主 API 契约版本（`manifest.rs:12-13`）。 |
| `runtime` | string | 是 | — | 目前仅 `"lua"`（`spec.rs:6`；校验 `manifest.rs:83-88`）。 |
| `main` | string | 是 | — | 实时入口脚本；`live.main` 缺失时作为回退（`manifest.rs:176-181`）。 |
| `permissions` | string[] | 否 | `[]` | 实时权限；`live.permissions` 缺失时作为回退（`manifest.rs:183-189`）。 |
| `description` | string | 否 | — | 一句话描述（市场元数据）。 |
| `author` | string | 否 | — | 作者。建议 `"名字 <邮箱>"` 或 GitHub 用户名。 |
| `homepage` | string | 否 | — | 主页 URL。 |
| `repository` | string | 否 | — | 源码仓库 URL。 |
| `license` | string | 否 | — | SPDX 标识，如 `MIT`、`Apache-2.0`。 |
| `category` | string | 否 | — | 分类标签，如 `gcode`、`chart`、`template`、`demo`。 |
| `icon` | string | 否 | — | 图标相对插件根目录的路径，建议 PNG/SVG。 |
| `contributes` | object | 否 | `{}` | 贡献项（命令、UI、面板、设置）。 |
| `live` | object | 否 | — | 实时插件配置（`LiveConfig`）。 |

市场元数据字段均为可选，仅供 UI 展示与市场索引（`manifest.rs:32-63`）。

`plugins/plugin.schema.json` 的 `required` 为 `id`、`name`、`version`、`runtime`、`main`。

## api_version

`api_version` 表示插件依赖的宿主 API 契约，不是插件自身版本。当前仅支持 `"0.1"`（`crates/extension/src/manifest.rs:9-10`）。缺省时按 `"0.1"` 处理；写其它值会得到 `unsupported_api_version` 警告诊断（Warning）且插件不被加载（`crates/extension/src/manager.rs:191-206`）。

## live 配置

```json
"live": {
  "main": "main.lua",
  "permissions": ["bus", "log", "serial", "ui", "storage", "timer"],
  "subscriptions": ["transport.serial.default.rx"]
}
```

| 字段 | 类型 | 必填 | 默认 | 说明 |
|---|---|---:|---|---|
| `main` | string | 否 | 顶层 `main` | 实时入口脚本。 |
| `permissions` | string[] | 否 | 顶层 `permissions` | 实时权限。 |
| `subscriptions` | string[] | 否 | `[]` | 声明订阅的 topic（元数据，`manifest.rs:228-238`）。 |

## 权限

宿主按权限挂载对应 `ctx.*` API，未在清单声明的权限不会挂载（`crates/lua_host/src/lib.rs:848-945`）。

实时插件可用权限（`crates/extension/src/spec.rs:7-19`）：

| 权限 | 挂载的 API | 能力 |
|---|---|---|
| `log` | `ctx.log.*` | 输出日志。 |
| `bus` | `ctx.bus.*` | 发布、查询、等待、监听事件。 |
| `serial` | `ctx.serial.*` | 打开/关闭串口、发送数据。 |
| `ui` | `ctx.ui.*` | 创建或移除动态面板、更新面板值。 |
| `timer` | `ctx.timer.*` | 定时任务。 |
| `storage` | `ctx.session.*` | 运行期键值存储。 |
| `dialog` | `ctx.dialog.open_file` | 请求宿主文件选择对话框。 |
| `fs.read.user_selected` | `ctx.fs.*` | 读取用户经对话框明确选择的文件。 |
| `task` | `ctx.task.*` | 可暂停、可取消的长任务。 |
| `config` | `ctx.config.*` | 读取与保存插件配置。 |
| `testing` | `ctx.test.*` | 测试辅助 API，普通插件不建议使用。 |

`ctx.commands` 不需要任何权限，始终挂载（`crates/lua_host/src/lib.rs:863-870`）。完整 API 清单见 [lua-api.md](lua-api.md)。

## contributes

`contributes` 下的 `commands`、`ui`、`panels`、`settings` 均为数组，默认空（`manifest.rs:296-308`）。

### contributes.commands

```json
{ "id": "my-plugin.send", "title": "插件发送" }
```

| 字段 | 类型 | 必填 | 说明 |
|---|---|---:|---|
| `id` | string | 是 | 命令 ID，非空且唯一。 |
| `title` | string | 是 | 命令显示名。 |

### contributes.panels

```json
{ "id": "my-plugin.chart", "title": "图表", "kind": "chart", "topic_prefix": "protocol.my-plugin." }
```

| 字段 | 类型 | 必填 | 默认 | 说明 |
|---|---|---:|---|---|
| `id` | string | 是 | — | 面板 ID，非空且唯一。 |
| `title` | string | 是 | — | 面板标题。 |
| `kind` | string | 否 | `"chart"` | 面板类型（`manifest.rs:372-374`）。 |

`kind` 合法值（`spec.rs:21`）：`chart`、`form`、`attitude`、`gauge`、`table`。

除 `id` / `title` / `kind` 外的字段会被原样合并进 `ui.panel.create`，因此声明式面板与动态面板共用同一协议（`manifest.rs:359-369`），例如 `topic`、`topic_prefix`、`auto_apply`、`fields`、`min` / `max` / `unit`、`zones`、`columns` 等。这些面板专属字段由 `plugins/plugin.schema.json` 补充描述。

### contributes.ui

插件只声明控件与命令，不直接绘制 egui。

```json
{
  "id": "my-plugin.send.button",
  "slot": "send.toolbar",
  "kind": "button",
  "title": "发送",
  "command": "my-plugin.send",
  "tooltip": "使用发送区当前内容执行插件动作",
  "order": 10,
  "record_send_input": true
}
```

| 字段 | 类型 | 必填 | 默认 | 说明 |
|---|---|---:|---|---|
| `id` | string | 是 | — | 控件 ID，非空且唯一。 |
| `slot` | string | 是 | — | 挂载插槽。 |
| `kind` | string | 否 | `"button"` | 控件类型（`manifest.rs:354-356`）。 |
| `title` | string | 否 | — | 显示文本。 |
| `command` | string | 否 | — | 点击时触发的命令，必须指向已声明的 `commands[].id`。 |
| `tooltip` | string | 否 | — | 悬停提示。 |
| `order` | integer | 否 | `0` | 同插槽内排序。 |
| `enabled` | boolean | 否 | `true` | 初始可用状态。 |
| `visible` | boolean | 否 | `true` | 初始可见状态。 |
| `record_send_input` | boolean | 否 | `false` | `send.*` 插槽下，点击时把当前发送区内容写入发送历史。 |
| `default` | any | 否 | — | `toggle` 初始状态 / `progress` 初始值。 |

`slot` 合法值（`spec.rs:22-28`）：`send.toolbar`、`top_bar.left`、`top_bar.right`、`status_bar.left`、`status_bar.right`。

`kind` 合法值（`spec.rs:29-37`）：`button`、`small_button`、`separator`、`label`、`status`、`toggle`、`progress`。

### contributes.settings

```json
{ "id": "ack_timeout_ms", "title": "ACK 超时", "kind": "number", "default": 300000, "step": 1000 }
```

| 字段 | 类型 | 必填 | 默认 | 说明 |
|---|---|---:|---|---|
| `id` | string | 是 | — | 设置项 ID。 |
| `title` | string | 是 | — | 设置项显示名。 |
| `kind` | string | 否 | `"text"` | 控件类型（`manifest.rs:376-378`）。 |
| `default` | any | 否 | — | 默认值。 |
| `options` | array | 否 | `[]` | `select` 等选项。 |
| `min` / `max` / `step` | number | 否 | — | 数值/滑块范围与步长。 |
| `rows` | integer | 否 | — | `textarea` 行数。 |
| `description` | string | 否 | — | 说明文本。 |

`kind` 合法值（`spec.rs:38-40`）：`text`、`number`、`boolean`、`checkbox`、`select`、`slider`、`textarea`、`serial`。

## 校验规则

加载时逐条校验（`crates/extension/src/manifest.rs:72-170`）；任一条不通过即产生 `manifest_validation_error` 诊断并且不加载插件（`crates/extension/src/manager.rs:208-218`）：

- `id` / `name` / `version` / `runtime` 非空，`runtime` 必须受支持，实时入口非空。
- `commands[].id` 非空且在 `commands` 内唯一。
- `ui[].id` 非空且在 `ui` 内唯一；`slot` 属于 `UI_SLOTS`；`kind` 属于 `UI_KINDS`；填写了 `command` 时必须指向已声明的命令。
- `panels[].id` 非空且在 `panels` 内唯一；`kind` 属于 `PANEL_KINDS`。
- `settings[].kind` 属于 `SETTING_KINDS`。

`commands[].id`、`ui[].id`、`panels[].id` 的 `id` 由 Schema 要求非空，`settings[].id` / `title` 同样为必填字段。

## 旧格式兼容

没有 `live` 时，系统用顶层 `main` 与 `permissions` 作为实时插件配置。

```json
{
  "id": "my.old-plugin",
  "name": "旧格式插件",
  "version": "0.1.0",
  "api_version": "0.1",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["bus", "log", "ui"]
}
```

## 与 Schema / 生成物的关系

- `plugins/plugin.schema.json` 是编辑器表面；其中的枚举由 `crates/extension/src/spec.rs` 通过 `cargo run -p tool-extension --example sync_plugin_spec` 同步（`spec.rs:91-116`）。
- [spec.generated.md](spec.generated.md) 是同一命令生成的协议枚举参考，**请勿手改**。

## 相关文档

- [README.md](README.md)——插件开发总览与目录结构。
- [lua-api.md](lua-api.md)——`ctx.*` API 与回调。
- [scenarios.md](scenarios.md)——场景测试 DSL。
- [spec.generated.md](spec.generated.md)——协议枚举生成物。
- [../ARCHITECTURE.md](../ARCHITECTURE.md)、[../known-gaps.md](../known-gaps.md)。

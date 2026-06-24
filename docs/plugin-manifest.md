# plugin.json 说明

`plugin.json` 是插件声明文件。主程序通过它识别插件 ID、入口脚本、权限、贡献的面板以及回放解析器。

开发模板会在文件顶部包含：

```json
"$schema": "../plugin.schema.json"
```

这个字段只给编辑器使用，主程序会忽略它。保持 `plugins\plugin.schema.json` 位于插件目录的上一级，可以获得字段提示和基础校验。

## 推荐完整示例

```json
{
  "$schema": "../plugin.schema.json",
  "id": "demo.signal-generator",
  "name": "信号发生器 (Demo)",
  "version": "1.0.0",
  "api_version": "0.1",
  "runtime": "lua",

  "main": "main.lua",
  "permissions": ["bus", "log", "serial", "ui", "storage", "timer"],

  "live": {
    "main": "main.lua",
    "permissions": ["bus", "log", "serial", "ui", "storage", "timer"]
  },

  "replay": {
    "main": "replay.lua",
    "subscriptions": ["transport.serial.default.rx"],
    "outputs": ["protocol.demo.sample"],
    "permissions": ["log", "storage"]
  },

  "contributes": {
    "commands": [
      { "id": "demo.signal-generator.start", "title": "开始输出" }
    ],
    "ui": [
      {
        "id": "demo.signal-generator.start.button",
        "slot": "send.toolbar",
        "kind": "button",
        "command": "demo.signal-generator.start",
        "tooltip": "从发送区工具栏触发插件动作",
        "order": 10
      }
    ],
    "panels": [
      { "id": "demo-signal-chart", "title": "信号波形", "kind": "chart" },
      { "id": "demo-signal-form", "title": "信号参数", "kind": "form" }
    ]
  }
}
```

## 顶层字段

| 字段 | 类型 | 必填 | 说明 |
|---|---:|---:|---|
| `id` | string | 是 | 插件唯一 ID。建议稳定，不要随意修改。 |
| `name` | string | 是 | UI 中显示的插件名称。 |
| `version` | string | 是 | 插件版本。 |
| `api_version` | string | 否 | 插件 API 版本。当前支持 `"0.1"`；旧插件不填时按 `"0.1"` 处理。 |
| `runtime` | string | 是 | 当前 Lua 插件使用 `"lua"`。 |
| `main` | string | 是 | 旧格式入口；没有 `live.main` 时作为实时入口。 |
| `permissions` | string[] | 否 | 旧格式实时权限；没有 `live.permissions` 时使用。 |
| `live` | object | 否 | 实时插件配置。 |
| `replay` | object | 否 | 回放解析器配置。 |
| `contributes` | object | 否 | 插件贡献项，例如面板声明。 |

## api_version

`api_version` 表示插件使用的宿主 API 契约版本，不是插件自身版本。主程序会在发现插件时检查它：

```json
"api_version": "0.1"
```

当前支持：

```text
0.1
```

旧插件可以不写 `api_version`，主程序会按 `0.1` 兼容处理。新插件建议显式写上，方便未来宿主升级 API 时给出清楚的兼容提示。

## live 配置

```json
"live": {
  "main": "main.lua",
  "permissions": ["bus", "log", "serial", "ui", "storage", "timer"]
}
```

## replay 配置

```json
"replay": {
  "main": "replay.lua",
  "subscriptions": ["transport.serial.default.rx"],
  "outputs": ["protocol.demo.sample"],
  "permissions": ["log", "storage"]
}
```

- `subscriptions`：解析器接收的 topic 前缀。
- `outputs`：解析器可能输出的 topic。
- `permissions`：当前建议只使用 `log` 和 `storage`。

## 权限说明

实时插件可用权限：

| 权限 | 能力 |
|---|---|
| `log` | 使用 `ctx.log.*` 输出日志。 |
| `bus` | 使用 `ctx.bus.*` 发布、查询、等待或监听事件。 |
| `serial` | 使用 `ctx.serial.*` 操作串口。 |
| `ui` | 使用 `ctx.ui.*` 创建或移除动态面板。 |
| `timer` | 使用 `ctx.timer.*` 创建定时任务。 |
| `storage` | 使用 `ctx.storage.*` 运行期存储。 |
| `dialog` | 使用 `ctx.dialog.open_file` 请求宿主文件选择对话框。 |
| `fs.read.user_selected` | 读取用户通过宿主对话框或受控 UI 明确选择的文件。 |
| `task` | 使用 `ctx.task.*` 运行可暂停、可取消的长任务。 |
| `config` | 使用 `ctx.config.*` 读取和保存插件配置。 |
| `testing` | 测试辅助 API，普通插件不建议使用。 |

回放解析器当前只建议声明：

```json
"permissions": ["log", "storage"]
```

## contributes.panels

```json
"contributes": {
  "panels": [
    { "id": "my-plugin.chart", "title": "图表", "kind": "chart" }
  ]
}
```

`kind` 常见值：

```text
chart
form
attitude
```

## contributes.ui

`contributes.ui` 用于把插件动作挂到宿主定义的 UI 插槽。插件只声明控件和命令，不直接绘制 egui。

`contributes.commands`、`contributes.ui` 和 `contributes.panels` 内部的 `id` 必须各自唯一。`contributes.ui[].command` 如果填写，必须指向同一清单中已经声明的 `contributes.commands[].id`。

```json
"contributes": {
  "commands": [
    { "id": "my-plugin.send", "title": "插件发送" }
  ],
  "ui": [
    {
      "id": "my-plugin.send.button",
      "slot": "send.toolbar",
      "kind": "button",
      "command": "my-plugin.send",
      "tooltip": "使用发送区当前内容执行插件动作",
      "record_send_input": true,
      "order": 10
    }
  ]
}
```

当前宿主提供的插槽：

```text
send.toolbar
```

当前受控控件类型：

```text
button
small_button
separator
label
status
```

`send.*` 插槽中的控件可以设置 `record_send_input: true`。点击时宿主会把当前发送区内容写入发送历史；插件仍只处理 command，不需要直接操作宿主 UI 状态。

点击 `button` / `small_button` 时，宿主会发布 `plugin.command.execute` 给 `ctx.commands.register` 注册的处理函数；同时保留发布 `ui.contribution.action`，兼容旧插件。payload 包含：

```json
{
  "plugin_id": "my-plugin",
  "contribution_id": "my-plugin.send.button",
  "slot": "send.toolbar",
  "command": "my-plugin.send",
  "action": "my-plugin.send",
  "context": {
    "send": {
      "input": "发送区文本",
      "target_port": "COM3",
      "target_port_open": true,
      "hex_mode": false,
      "line_ending": { "label": "LF", "suffix": "\n" }
    }
  }
}
```

新 Lua 插件应使用 `ctx.commands.register("my-plugin.send", handler)`。旧插件仍可监听 `ui.contribution.action`，并先检查 `payload.plugin_id` 是否等于自己的插件 ID。

## 旧格式兼容

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

如果没有 `live`，系统会用顶层 `main` 和 `permissions` 作为实时插件配置。没有 `replay` 时，该插件不支持回放重解析。

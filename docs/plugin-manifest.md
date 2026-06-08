# plugin.json 说明

`plugin.json` 是插件声明文件。主程序通过它识别插件 ID、入口脚本、权限、贡献的面板以及回放解析器。

## 推荐完整示例

```json
{
  "id": "demo.signal-generator",
  "name": "信号发生器 (Demo)",
  "version": "1.0.0",
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
| `runtime` | string | 是 | 当前 Lua 插件使用 `"lua"`。 |
| `main` | string | 是 | 旧格式入口；没有 `live.main` 时作为实时入口。 |
| `permissions` | string[] | 否 | 旧格式实时权限；没有 `live.permissions` 时使用。 |
| `live` | object | 否 | 实时插件配置。 |
| `replay` | object | 否 | 回放解析器配置。 |
| `contributes` | object | 否 | 插件贡献项，例如面板声明。 |

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

## 旧格式兼容

```json
{
  "id": "my.old-plugin",
  "name": "旧格式插件",
  "version": "0.1.0",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["bus", "log", "ui"]
}
```

如果没有 `live`，系统会用顶层 `main` 和 `permissions` 作为实时插件配置。没有 `replay` 时，该插件不支持回放重解析。

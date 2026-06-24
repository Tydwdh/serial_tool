# Hello 插件模板

这是最小插件模板，展示：

- `plugin.json` 基本结构
- `ctx.log`
- `ctx.ui.create_form`
- `ctx.bus.on("ui.form.changed", ...)`
- `ctx.storage.get/set`
- `on_disable`

## 使用方法

1. 复制目录：

```text
plugins/template.hello -> plugins/yourname.hello
```

2. 修改 `plugin.json`：

```json
"id": "yourname.hello",
"name": "你的 Hello 插件"
```

保留 `$schema` 字段，它会让支持 JSON Schema 的编辑器提示可用字段和权限。

3. 修改 `main.lua` 里的 `PANEL_ID`，避免面板 ID 冲突。

4. 在插件页刷新或重启应用后启用插件。

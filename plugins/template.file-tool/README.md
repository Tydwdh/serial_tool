# 文件工具插件模板

这个模板展示：

- `plugin.json` 的文件选择权限声明
- `ctx.dialog.open_file`
- `ctx.ui.create_form`
- `ctx.ui.set_value`
- `ctx.ui.set_enabled`
- `ctx.storage.get/set`

## 使用方法

1. 复制目录：

```text
plugins/template.file-tool -> plugins/yourname.file-tool
```

2. 修改 `plugin.json`：

```json
"id": "yourname.file-tool",
"name": "你的文件工具"
```

保留 `$schema` 字段，它会让支持 JSON Schema 的编辑器提示可用字段和权限。

3. 修改 `main.lua` 里的面板 ID，避免和其他插件冲突。

4. 在插件页刷新或重启应用后启用插件。

## 权限说明

这个模板声明了：

```json
"dialog",
"fs.read.user_selected"
```

插件只能读取用户通过宿主文件对话框或受控 UI 明确选择的文件。不要把任意本地路径读取做成默认行为。

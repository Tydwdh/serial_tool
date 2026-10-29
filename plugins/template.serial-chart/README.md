# 串口图表插件模板

这个模板展示完整链路：

```text
串口 RX -> main.lua -> protocol.template.sample -> 图表
```

## 输入数据格式

默认解析一行 JSON 文本：

```json
{"seq":1,"value":12.3,"target":50.0}
```

建议带换行符：

```text
{"seq":1,"value":12.3,"target":50.0}\n
```

## 使用方法

1. 复制目录：

```text
plugins/template.serial-chart -> plugins/yourname.serial-chart
```

2. 修改 `plugin.json`：

```json
"id": "yourname.serial-chart",
"name": "你的串口图表插件"
```

保留 `$schema` 字段，它会让支持 JSON Schema 的编辑器提示可用字段、权限和面板配置。

项目根目录的 `.luarc.json` 会加载 `plugins\.lua\` 下的 Lua API stub，用于消除 `ctx` 和 `require("hw.*")` 的假报错。

3. 修改 `main.lua` 中的：

```lua
CHART_ID
FORM_ID
TOPIC_OUT
```

4. 根据你的协议修改 `parse_packet()`。

## 运行验证

1. 在插件页启用本插件。
2. 打开一个串口，让该串口收到一行 `{"seq":1,"value":12.3,"target":50.0}\n` 数据（或把 `rx_port` 设为该端口）。
3. 图表应出现一条采样点；插件日志显示解析成功。
4. 修改参数面板中的 `rx_port`，只保留目标端口的数据。

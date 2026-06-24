# 串口图表插件模板

这个模板展示完整链路：

```text
实时:
  串口 RX -> main.lua -> protocol.template.sample -> 图表

回放:
  RawSerial 录制 -> replay.lua -> protocol.template.sample -> 图表
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

保留 `$schema` 字段，它会让支持 JSON Schema 的编辑器提示可用字段、权限和回放配置。

3. 修改 `main.lua` 和 `replay.lua` 中的：

```lua
CHART_ID
FORM_ID
TOPIC_OUT
```

4. 根据你的协议修改 `parse_packet()`。

## 录制与回放测试

### StandardReplay

1. 录制模式选择“标准回放”。
2. 产生串口数据。
3. 停止录制。
4. 选择“自动”或“精确回放”。
5. 图表应直接恢复。

### RawSerial + ReparseRaw

1. 录制模式选择“原始串口”。
2. 产生串口数据。
3. 停止录制。
4. 回放策略选择“重新解析”。
5. replay.lua 应生成 `protocol.template.sample`，图表应显示数据。

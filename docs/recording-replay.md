# 录制与回放

## 录制模式 RecordMode

### 原始串口 RawSerial

只记录：

```text
transport.serial.*
```

适合：

- 只查看串口收发历史。
- 文件尽量小。
- 回放时用 `replay.lua` 重新生成图表。

限制：如果不使用 `ReparseRaw`，图表不会自动恢复。

### 标准回放 StandardReplay

默认推荐模式。记录：

```text
transport.serial.*
protocol.*
ui.panel.create
```

适合：

- 回放终端接收区。
- 回放图表结果。
- 恢复动态图表面板。
- 不依赖当前插件版本重新解析。

### 完整调试 FullDebug

记录几乎所有 live event，但排除：

```text
metadata.replay == true
metadata.origin == "replay"
metadata.origin == "replay_derived"
metadata.recordable == false
```

适合排查系统级问题。缺点是文件最大。

## 回放策略 ReplayPolicy

### 自动 AutoPreferRecorded

默认推荐策略。

```text
如果录制文件中存在 protocol.*：
    使用精确回放
否则：
    尝试重新解析
```

注意：当前版本的自动判断主要基于是否存在 `protocol.*`。后续可扩展为按图表 topic 精细判断。

### 精确回放 ExactRecorded

使用录制文件中的 `protocol.*` 事件，不运行 `replay.lua`。

优点：最快、最稳定、图表和录制时结果一致。

### 重新解析 ReparseRaw

忽略录制文件里的 `protocol.*`，使用 `replay.lua` 从 `transport.serial.*` 重新生成 `protocol.*`。

优点：

- 支持 RawSerial 文件显示图表。
- 可以用新版解析器重新分析旧数据。

缺点：

- 结果可能和录制时不同。
- 大文件可能需要等待 analyzer 运行。

## 推荐组合

| 录制模式 | 回放策略 | 效果 |
|---|---|---|
| StandardReplay | AutoPreferRecorded | 推荐默认。接收区和图表都能恢复。 |
| StandardReplay | ExactRecorded | 明确使用录制时结果。 |
| RawSerial | ReparseRaw | 文件小，回放时用 replay.lua 生成图表。 |
| FullDebug | ExactRecorded | 排查系统问题。 |

## 为什么 live plugin 不处理 replay event

实时插件可能会：

- 打开串口
- 发送串口数据
- 启动 timer
- 创建 UI
- 再次发布 `protocol.*`

如果回放时把 replay RX 事件送给 live plugin，可能导致图表出现重复数据或状态污染。

设计规则：

```text
live plugin 不处理 replay event
replay analyzer 通过独立路径处理 replay raw event
```

## StandardReplay 的动态图表恢复

`StandardReplay` 会记录 `ui.panel.create`。回放重建时应先发布 panel create 事件，让动态面板创建完成，再发布 `protocol.*` 数据事件。这样图表订阅才能接住后续数据。

## 发布前测试建议

### StandardReplay 精确回放

1. 启用带图表的插件。
2. 录制模式选择“标准回放”。
3. 产生一些串口数据和图表数据。
4. 停止录制。
5. 重启应用或关闭插件。
6. 加载录制文件。
7. 策略选择“自动”或“精确回放”。
8. 确认图表恢复且数据不重复。

### RawSerial 重新解析

1. 录制模式选择“原始串口”。
2. 产生串口数据。
3. 停止录制。
4. 加载文件。
5. 策略选择“重新解析”。
6. 确认 replay.lua 生成图表数据。

### 不重复 protocol 数据

确认同一个 `protocol.demo.sample` 不会同时来自：

```text
录制文件里的 protocol.demo.sample
replay.lua 重新生成的 protocol.demo.sample
```

除非未来显式实现“对比模式”，默认不应混用。

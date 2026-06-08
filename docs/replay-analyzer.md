# Replay Analyzer 回放解析器

`replay.lua` 是插件的回放解析器。它用于“只录原始串口数据，回放时重新解析出图表”的场景。

## 为什么需要 replay.lua

实时插件 `main.lua` 可以打开串口、发送数据、创建 UI、运行 timer。这些行为在回放阶段不应该发生。

因此回放阶段使用独立的受限环境：

```text
main.lua   = 实时插件，有副作用
replay.lua = 回放解析器，无副作用
```

## 典型使用场景

### StandardReplay

录制内容：

```text
transport.serial.*
protocol.*
ui.panel.create
```

回放时直接使用录制文件里的 `protocol.*`，不需要重新运行解析器。

### RawSerial + ReparseRaw

录制内容：

```text
transport.serial.*
```

回放时没有 `protocol.*`，因此需要 `replay.lua` 从原始串口 RX 重建图表事件。

## plugin.json 中声明 replay

```json
"replay": {
  "main": "replay.lua",
  "subscriptions": ["transport.serial.default.rx"],
  "outputs": ["protocol.demo.sample"],
  "permissions": ["log", "storage"]
}
```

## 可用 API

Replay Analyzer 中可用：

```lua
ctx.plugin
ctx.storage.get(key)
ctx.replay.emit(topic, payload)
ctx.replay.log(message)
ctx.replay.current_event()
ctx.now_ms()
```

不可用：

```text
ctx.serial
ctx.timer
ctx.ui
ctx.bus.publish
ctx.bus.on
ctx.storage.set
```

## 生命周期

```lua
function on_replay_begin(session)
end

function on_replay_event(event)
end

function on_replay_end()
end
```

`session` 常用字段：

```lua
session.start_ms
session.end_ms
session.event_count
```

## 输出事件

```lua
ctx.replay.emit("protocol.demo.sample", {
  t = 1,
  target = 50.0,
  actual = 49.7,
  output = 0.12
})
```

宿主会自动为输出事件添加 metadata：

```json
{
  "replay": true,
  "origin": "replay_derived",
  "category": "derived",
  "derived": true,
  "plugin_id": "...",
  "plugin_version": "...",
  "derived_from": [123],
  "recordable": false
}
```

这些事件不会再次被 recorder 写入录制文件。

## replay.lua 示例

```lua
local buffers = {}
local received = 0
local lost_total = 0
local last_seq = nil

local function reset()
  buffers = {}
  received = 0
  lost_total = 0
  last_seq = nil
end

local function parse_packet(line)
  local seq = tonumber(line:match('"seq":(%d+)'))
  local actual = tonumber(line:match('"actual":([%d.-]+)'))
  local target = tonumber(line:match('"target":([%d.-]+)'))
  local output = tonumber(line:match('"output":([%d.-]+)'))

  if not seq or not actual or not target or not output then
    return nil
  end

  return { seq = seq, actual = actual, target = target, output = output }
end

local function emit_sample(sample)
  received = received + 1

  if last_seq and sample.seq > last_seq + 1 then
    lost_total = lost_total + (sample.seq - last_seq - 1)
  end

  last_seq = sample.seq

  ctx.replay.emit("protocol.demo.sample", {
    t = sample.seq,
    target = sample.target,
    actual = sample.actual,
    output = sample.output,
    rx_total = received,
    lost_total = lost_total
  })
end

local function feed(port, text)
  local current = buffers[port] or ""
  current = current .. text

  while true do
    local start_index, end_index = current:find("\n", 1, true)
    if not start_index then break end

    local line = current:sub(1, start_index - 1):gsub("\r", "")
    current = current:sub(end_index + 1)

    local sample = parse_packet(line)
    if sample then emit_sample(sample) end
  end

  buffers[port] = current
end

function on_replay_begin(session)
  reset()
  ctx.replay.log("replay analyzer started, events=" .. tostring(session.event_count))
end

function on_replay_event(event)
  if event.topic ~= "transport.serial.default.rx" then return end

  local port = "default"
  if event.metadata and event.metadata.port then
    port = tostring(event.metadata.port)
  end

  feed(port, tostring(event.payload))
end

function on_replay_end()
  ctx.replay.log("replay analyzer finished, received=" .. tostring(received))
end
```

## 设计注意事项

- 保持无副作用：不要操作串口、创建 UI、启动 timer。
- 处理粘包和拆包：串口 RX 不一定刚好是一行完整数据。
- 不要依赖实时状态：Replay Analyzer 只依赖历史事件和只读配置。
- 版本变化可能导致重新解析结果和录制时不同，应在插件 README 中说明。

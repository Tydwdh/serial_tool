# Lua 插件开发文档

## 快速开始

插件是一个包含 `plugin.json` 和 `main.lua` 的目录，放在 `plugins/` 下。

```
plugins/my-plugin/
├── plugin.json
└── main.lua
```

### plugin.json

```json
{
  "id": "my-plugin",
  "name": "我的插件",
  "version": "1.0.0",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["bus", "log", "serial", "ui", "storage", "timer", "testing"],
  "contributes": {
    "panels": [
      { "id": "my-panel", "title": "数据面板", "kind": "chart" }
    ]
  }
}
```

### main.lua（一次性模式）

```lua
-- 创建一个图表面板
ctx.ui.create_chart({
    id = "my-panel",
    title = "PID 数据",
    topic_prefix = "protocol.pid."
})
```

### main.lua（事件驱动模式）

```lua
-- 注册事件回调
ctx.bus.on("protocol.pid.sample", function(event)
    ctx.log.info(string.format("收到: actual=%.2f", event.payload.actual))
end)

-- 每秒发送心跳
ctx.timer.every(1000, function()
    ctx.bus.publish("plugin.heartbeat", { t = ctx.now_ms() })
end)

-- 禁用时清理
on_disable(function()
    ctx.ui.remove_panel("my-panel")
end)
```

---

## API 参考

### ctx.log — 日志

```lua
ctx.log.trace("调试信息")
ctx.log.debug("调试信息")
ctx.log.info("普通信息")
ctx.log.warn("警告信息")
ctx.log.error("错误信息")
```

---

### ctx.bus — 数据总线

#### 发布事件

```lua
ctx.bus.publish("protocol.pid.sample", {
    t = 1, target = 50.0, actual = 43.0, output = 0.71
})
```

payload 支持: `table`（自动转 JSON）、`string`、`number`、`boolean`、`nil`

#### 读取历史

```lua
local events = ctx.bus.history("protocol.")  -- 最近 100 条匹配前缀的事件
for _, event in ipairs(events) do
    ctx.log.info(event.topic .. ": " .. tostring(event.payload))
end
```

#### 阻塞等待

```lua
-- 等待精确 topic（一次性脚本使用）
local event = ctx.bus.wait("test.ready", 1000)  -- 超时毫秒
if event then
    ctx.log.info("收到: " .. event.payload)
end

-- 等待前缀匹配
local event = ctx.bus.subscribe("protocol.", 5000)
```

#### 事件回调（事件驱动模式）

```lua
-- 注册回调（支持前缀匹配）
ctx.bus.on("protocol.pid.sample", function(event)
    -- event.id, event.timestamp_ms, event.topic, event.source
    -- event.payload, event.metadata
    ctx.log.info(string.format("PID: %.2f", event.payload.actual))
end)

-- 前缀匹配
ctx.bus.on("protocol.", function(event)
    -- 匹配所有 protocol.* 事件
end)

-- 取消回调
ctx.bus.off("protocol.pid.sample")
```

---

### ctx.serial — 串口

#### 枚举端口

```lua
local ports = ctx.serial.list()
for _, port in ipairs(ports) do
    ctx.log.info(port.port_name .. " (" .. port.port_type .. ")")
end
```

#### 打开/关闭

```lua
-- 完整配置
ctx.serial.open({
    port_name = "COM3",
    baud_rate = 115200,
    data_bits = "8",       -- "5"|"6"|"7"|"8"
    stop_bits = "1",       -- "1"|"2"
    parity = "none",       -- "none"|"odd"|"even"
    timeout_ms = 50
})

-- 简写
ctx.serial.open("COM3")

ctx.serial.close()
```

#### 发送

```lua
ctx.serial.send("AT\r\n")
ctx.serial.send_hex("41 54 0D 0A")
```

#### 状态

```lua
local status = ctx.serial.status()
-- { open = true, port_name = "COM3", baud_rate = 115200 }
```

#### 等待数据

```lua
-- 等待匹配文本（阻塞）
local line = ctx.serial.expect("OK", 5000)
if line then
    ctx.log.info("收到: " .. line)
end
```

---

### ctx.timer — 定时器

```lua
-- 延时执行一次
local id = ctx.timer.after(500, function()
    ctx.log.info("500ms 后执行")
end)

-- 周期性执行
local id = ctx.timer.every(1000, function()
    ctx.bus.publish("tick", { time = ctx.now_ms() })
end)

-- 取消
ctx.timer.cancel(id)
```

---

### ctx.storage — 持久化存储

```lua
-- 写入
ctx.storage.set("last_value", "42")

-- 读取（不存在返回 nil）
local value = ctx.storage.get("last_value")

-- 列出所有键
local keys = ctx.storage.keys()
for _, key in ipairs(keys) do
    ctx.log.info(key .. " = " .. ctx.storage.get(key))
end
```

**注意：** 存储只在插件运行期间有效。重启后数据丢失。如需跨启动持久化，将数据发布为事件并写入日志文件。

---

### ctx.ui — 面板

#### 创建面板

```lua
-- 图表
ctx.ui.create_chart({
    id = "pid-chart",
    title = "PID 曲线",
    topic_prefix = "protocol.pid."
})

-- 表单
ctx.ui.create_form({
    id = "pid-form",
    title = "PID 参数",
    fields = {
        { id = "kp", label = "Kp", kind = "number", default = 1.0 },
        { id = "ki", label = "Ki", kind = "number", default = 0.0 },
        { id = "kd", label = "Kd", kind = "number", default = 0.0 },
    }
})

-- 3D 姿态
ctx.ui.create_attitude({
    id = "imu-view",
    title = "IMU 姿态",
    topic = "protocol.imu.attitude"
})

-- 移除面板
ctx.ui.remove_panel("pid-chart")

-- 查询面板信息
local panel = ctx.ui.get_panel("pid-chart")
```

---

### ctx.test — 测试框架

```lua
test.before_each(function()
    ctx.log.info("每个用例前执行")
end)

test.after_each(function()
    ctx.log.info("每个用例后执行")
end)

test.case("基本断言", function()
    test.assert(1 + 1 == 2, "数学崩了")
    test.log("自定义日志")
end)

test.case("等待串口数据", function()
    local line = ctx.serial.expect("READY", 1000)
    test.assert(line ~= nil, "未收到 READY")
end)

test.case("验证总线事件", function()
    local event = test.expect("protocol.pid.sample", 1000)
    test.assert(event ~= nil, "未收到 PID 事件")
    test.assert(event.payload.target == 50, "target 不正确")
end)
```

---

### 辅助

```lua
-- 当前时间戳 (毫秒)
local ms = ctx.now_ms()

-- 插件元数据（只读）
local id = ctx.plugin.id
local name = ctx.plugin.name
local version = ctx.plugin.version
local root = ctx.plugin.root
local permissions = ctx.plugin.permissions
```

---

### 禁用回调

```lua
on_disable(function()
    -- 插件被禁用时调用
    ctx.ui.remove_panel("my-panel")
    ctx.bus.off("protocol.pid.sample")
end)
```

---

## 权限

| 权限 | 对应 API |
|------|----------|
| `bus` | ctx.bus.* |
| `log` | ctx.log.* |
| `serial` | ctx.serial.* |
| `ui` | ctx.ui.* |
| `timer` | ctx.timer.* |
| `storage` | ctx.storage.* |
| `testing` | ctx.test.* |

权限在 `plugin.json` 的 `permissions` 数组中声明。

---

## 完整示例

### 插件：PID 数据采集

```lua
-- plugins/pid-collector/main.lua

local count = 0

ctx.ui.create_chart({
    id = "pid-collector-chart",
    title = "PID 实时数据",
    topic_prefix = "protocol.pid."
})

-- 监听 PID 数据
ctx.bus.on("protocol.pid.", function(event)
    count = count + 1
    ctx.storage.set("sample_count", tostring(count))
end)

-- 每 5 秒报告统计
ctx.timer.every(5000, function()
    ctx.log.info(string.format("已采集 %d 条 PID 数据", count))
end)

-- 清除
on_disable(function()
    ctx.ui.remove_panel("pid-collector-chart")
    ctx.bus.off("protocol.pid.")
end)
```

```json
// plugins/pid-collector/plugin.json
{
  "id": "pid-collector",
  "name": "PID 采集器",
  "version": "1.0.0",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["bus", "log", "ui", "storage", "timer"],
  "contributes": {
    "panels": [
      { "id": "pid-collector-chart", "title": "PID 实时数据", "kind": "chart" }
    ]
  }
}
```

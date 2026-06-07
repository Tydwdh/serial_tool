# 硬件调试工作台 — 插件开发指南

## 目录

1. [架构概览](#1-架构概览)
2. [快速开始](#2-快速开始)
3. [Lua 插件开发](#3-lua-插件开发)
4. [WASM 插件开发](#4-wasm-插件开发)
5. [WASM 解码器开发](#5-wasm-解码器开发)
6. [Python 工具开发](#6-python-工具开发)
7. [DataBus 与 Topic 规范](#7-databus-与-topic-规范)
8. [测试框架](#8-测试框架)
9. [插件清单参考](#9-插件清单参考)
10. [权限系统](#10-权限系统)

---

## 1. 架构概览

```
┌─────────────────────────────────────────────────────────────┐
│                      硬件调试工作台                           │
│                                                             │
│  ┌──────────┐   ┌──────────┐   ┌──────────┐   ┌─────────┐ │
│  │ 串口接收  │   │ 图表可视化 │   │ 会话回放  │   │ 测试报告 │ │
│  │ (Terminal)│   │ (Chart)  │   │ (Replay) │   │ (Tests) │ │
│  └─────┬─────┘   └─────┬────┘   └────┬─────┘   └────┬────┘ │
│        │               │             │              │       │
│        └───────────────┴──────┬──────┴──────────────┘       │
│                               │                              │
│                        ┌──────▼──────┐                      │
│                        │   DataBus   │  ← 核心事件总线       │
│                        └──────┬──────┘                      │
│               ┌───────────────┼───────────────┐             │
│        ┌──────▼──────┐ ┌──────▼──────┐ ┌──────▼──────┐     │
│        │  Lua 运行时  │ │ WASM 运行时 │ │ Python 运行时│     │
│        │  (mlua)     │ │ (wasmtime) │ │ (子进程)    │     │
│        └──────┬──────┘ └──────┬──────┘ └──────┬──────┘     │
│               │               │               │             │
│        ┌──────▼───────────────▼───────────────▼──────┐     │
│        │              插件管理器 (PluginManager)      │     │
│        │   发现 → 权限检查 → 加载 → 启用 → 管理       │     │
│        └─────────────────────────────────────────────┘     │
│                                                             │
│        串口 ←→ TransportManager ←→ DataBus ←→ 录制/回放     │
└─────────────────────────────────────────────────────────────┘
```

**核心理念：**

| 层次 | 说明 |
|------|------|
| **入口** | 串口（USB-TTL / 蓝牙 / PCI），自动热插拔检测 |
| **总线** | DataBus — 发布/订阅事件总线，Topic 路由，20,000 条环形历史 |
| **扩展** | 插件系统 — 支持 Lua、WASM、Python 三种运行时 |
| **运行时** | Lua 5.4（脚本热加载）、Wasmtime（沙箱安全）、Python（子进程） |
| **体验** | 实时可视化（图表/姿态）、测试框架、会话录制与回放 |

---

## 2. 快速开始

### 2.1 目录结构

```
your-project/
├── plugins/                    # 插件目录（自动发现）
│   └── my-plugin/              # 一个插件 = 一个文件夹
│       ├── plugin.json         # 插件清单（必需）
│       ├── main.lua            # Lua 入口脚本
│       └── README.md           # 说明文档（可选）
├── wasm-decoders/              # WASM 解码器目录
│   └── my-decoder/             # 一个解码器 = 一个文件夹
│       ├── decoder.json        # 解码器清单（必需）
│       └── decoder.wat         # WAT/WASM 模块（必需）
├── tools/                      # Python 工具目录
│   └── my-tool/
│       ├── my_tool.py          # Python 脚本
│       └── README.md
├── scripts/                    # Lua 脚本保存目录
│   └── my-script.lua
└── logs/                       # JSONL 会话日志
    └── session-*.jsonl
```

### 2.2 插件生命周期

```
发现 (Discovered) → 权限检查 → 启用 (Enabled/Running)
                                  │
                                  ├─ 完成 (Finished)
                                  ├─ 失败 (Failed)
                                  └─ 禁用 (Disabled)
```

1. 启动时扫描 `plugins/` 下所有含 `plugin.json` 的子目录
2. 用户通过 UI 点击"启用"按钮
3. Lua 插件立即执行 `main.lua`（脚本运行完后自动标记 Finished）
4. WASM 插件调用 `activate` 导出函数，随后 `on_event` 响应事件
5. 用户可随时"禁用"插件

---

## 3. Lua 插件开发

Lua 插件是最简单的扩展方式。每个插件运行在独立线程中，通过 `ctx` 全局对象与系统交互。

### 3.1 ctx API 参考

#### ctx.log — 日志

```lua
ctx.log.trace("trace message")
ctx.log.debug("debug message")
ctx.log.info("info message")
ctx.log.warn("warn message")
ctx.log.error("error message")
```

消息发布到 `log.system` topic，显示在底部「日志」面板中。

#### ctx.bus — DataBus

```lua
-- 发布事件（自动以 plugin:<id> 为 source）
ctx.bus.publish("protocol.pid.sample", {
    t = 1,
    target = 50,
    actual = 43,
    output = 0.71
})

-- 等待指定 topic 的事件（阻塞，带超时）
local event = ctx.bus.wait("transport.serial.default.rx", 5000)  -- timeout_ms
if event then
    ctx.log.info("收到: " .. event.payload)
end

-- 等待前缀匹配的事件
local event = ctx.bus.subscribe("protocol.", 3000)

-- 查询历史事件（最近 100 条，按时间倒序）
local history = ctx.bus.history("transport.serial.")
for _, event in ipairs(history) do
    ctx.log.info(string.format("#%d %s", event.id, event.topic))
end
```

#### ctx.serial — 串口

```lua
-- 列出可用串口
local ports = ctx.serial.list()
for _, port in ipairs(ports) do
    ctx.log.info(port.port_name .. " (" .. port.port_type .. ")")
end

-- 打开串口（字符串 = 端口名）
ctx.serial.open("COM3")

-- 打开串口（Table = 完整配置）
ctx.serial.open({
    port_name = "COM3",
    baud_rate = 115200,
    timeout_ms = 50
})

-- 发送文本
ctx.serial.send("AT\r\n")

-- 发送 HEX
ctx.serial.send_hex("41 54 0D 0A")

-- 等待接收匹配（阻塞）
local line = ctx.serial.expect("OK", 5000)
if line then
    ctx.log.info("收到响应: " .. line)
end

-- 获取串口状态
local status = ctx.serial.status()
ctx.log.info("已连接: " .. tostring(status.open))

-- 关闭串口
ctx.serial.close()
```

#### ctx.ui — 动态面板

```lua
-- 创建图表面板（订阅 topic_prefix 下的所有 JSON 事件）
ctx.ui.create_chart({
    id = "my-plugin.chart",          -- 全局唯一 ID
    title = "My Chart",
    topic_prefix = "protocol.pid."   -- 数据源 topic 前缀
})

-- 创建表单面板
ctx.ui.create_form({
    id = "my-plugin.form",
    title = "My Parameters",
    fields = {
        { id = "kp", label = "Kp", kind = "number", default = 1.0 },
        { id = "ki", label = "Ki", kind = "number", default = 0.0 },
        { id = "enabled", label = "Enabled", kind = "boolean", default = true }
    }
})

-- 创建 3D 姿态面板
ctx.ui.create_attitude({
    id = "my-plugin.attitude",
    title = "IMU Attitude",
    topic = "protocol.imu.attitude"
})

-- 查询已创建的面板
local panel = ctx.ui.get_panel("my-plugin.chart")
if panel then
    ctx.log.info("面板标题: " .. panel.title)
end
```

面板创建后，用户在侧边栏可以看到对应标签页。**表单的「应用」按钮**会将表单数据发布到 `ui.form.changed` topic。

#### ctx.plugin — 插件元信息

```lua
-- 插件清单信息（由 plugin.json 注入）
local id = ctx.plugin.id            -- "builtin.pid-tuner"
local name = ctx.plugin.name        -- "PID Tuner"
local version = ctx.plugin.version  -- "0.1.0"
local runtime = ctx.plugin.runtime  -- "lua"
local root = ctx.plugin.root        -- 插件目录绝对路径
local permissions = ctx.plugin.permissions  -- {"bus", "log", "serial", "ui"}
```

### 3.2 超时保护

每个 Lua 脚本有执行时限（默认 60 秒）。通过指令计数器每 10,000 条指令检查一次。超时后脚本被强制终止并标记为 Failed。

测试用例的超时可通过 `test.timeout(ms)` 设置。

### 3.3 完整示例：PID Tuner

```json
// plugin.json
{
  "id": "builtin.pid-tuner",
  "name": "PID Tuner",
  "version": "0.1.0",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["bus", "log", "serial", "ui"],
  "contributes": {
    "commands": [
      { "id": "builtin.pid-tuner.apply", "title": "Apply PID Parameters" }
    ],
    "panels": [
      { "id": "builtin.pid-tuner.chart", "title": "PID Chart", "kind": "chart" },
      { "id": "builtin.pid-tuner.form", "title": "PID Parameters", "kind": "form" }
    ]
  }
}
```

```lua
-- main.lua

-- 解析串口收到的 PID 数据行
local function parse_pid_line(line)
  local sample = {}
  for key, value in string.gmatch(line, "([%a_]+)%s*=%s*([-+]?%d+%.?%d*)") do
    sample[key] = tonumber(value)
  end
  if sample.t and sample.target and sample.actual then
    ctx.bus.publish("protocol.pid.sample", {
      t = sample.t,
      target = sample.target,
      actual = sample.actual,
      output = sample.out or sample.output or 0
    })
    return true
  end
  return false
end

ctx.log.info("PID Tuner activated: " .. ctx.plugin.id)

-- 创建图表和表单面板
ctx.ui.create_chart({
  id = "builtin.pid-tuner.chart",
  title = "PID Chart",
  topic_prefix = "protocol.pid."
})

ctx.ui.create_form({
  id = "builtin.pid-tuner.form",
  title = "PID Parameters",
  fields = {
    { id = "kp", label = "Kp", kind = "number", default = 1.0 },
    { id = "ki", label = "Ki", kind = "number", default = 0.0 },
    { id = "kd", label = "Kd", kind = "number", default = 0.0 },
    { id = "enabled", label = "Enabled", kind = "boolean", default = true }
  }
})

-- 从历史数据中恢复最近的数据
local history = ctx.bus.history("transport.serial.")
for _, event in ipairs(history) do
  if type(event.payload) == "string" and parse_pid_line(event.payload) then
    -- 解析成功，图表会自动更新
  end
end

-- 发布示例数据做演示
ctx.bus.publish("protocol.pid.sample", {
  t = 100, target = 50, actual = 43, output = 0.71
})
```

---

## 4. WASM 插件开发

WASM 插件提供更强的沙箱隔离和性能保证。使用 WAT（WebAssembly Text Format）或 WASM 二进制格式。

### 4.1 Host 导入函数

WASM 模块通过 `host` 命名空间导入以下函数：

| 函数 | 签名 | 权限 | 说明 |
|------|------|------|------|
| `host.log` | `(ptr: i32, len: i32) -> ()` | `log` | 发布 Info 级别日志 |
| `host.bus_publish` | `(ptr: i32, len: i32) -> ()` | `bus` | 发布事件（JSON 格式） |
| `host.bus_subscribe` | `(ptr: i32, len: i32) -> ()` | `bus` | 订阅 topic（接收事件） |
| `host.ui_panel_create` | `(ptr: i32, len: i32) -> ()` | `ui` | 创建动态面板（JSON） |
| `host.storage_set` | `(key_ptr, key_len, val_ptr, val_len: i32) -> ()` | `storage` | 存储键值对 |
| `host.storage_get` | `(key_ptr, key_len, out_ptr, out_cap: i32) -> i32` | `storage` | 读取键值对，返回字节数 |

### 4.2 导出函数

| 函数 | 签名 | 生命周期 |
|------|------|---------|
| `activate` | `() -> i32` | 启用时调用一次 |
| `deactivate` | `() -> i32` | 禁用时调用一次 |
| `on_event` | `(ptr: i32, len: i32) -> i32` | 每个匹配事件调用 |

返回值约定：`0` = 成功，负数 = 错误码。

### 4.3 bus_publish JSON 格式

```json
{
  "topic": "protocol.wasm.plugin",
  "payload": { "activated": true },
  "metadata": { "key": "value" }
}
```

### 4.4 完整示例：WASM Demo

```json
// plugin.json
{
  "id": "builtin.wasm-demo",
  "name": "WASM Demo",
  "version": "0.1.0",
  "runtime": "wasm",
  "main": "main.wat",
  "permissions": ["bus", "log", "ui", "storage"],
  "contributes": {
    "panels": [
      { "id": "builtin.wasm-demo.chart", "title": "WASM Demo", "kind": "chart" }
    ],
    "subscriptions": [
      { "topic": "transport.serial.default.rx" }
    ]
  }
}
```

```wat
;; main.wat
(module
  (import "host" "log" (func $log (param i32 i32)))
  (import "host" "bus_publish" (func $bus_publish (param i32 i32)))
  (import "host" "bus_subscribe" (func $bus_subscribe (param i32 i32)))
  (import "host" "ui_panel_create" (func $ui_panel_create (param i32 i32)))
  (import "host" "storage_set" (func $storage_set (param i32 i32 i32 i32)))
  (memory (export "memory") 1)

  ;; 数据段：预填充字符串
  (data (i32.const 1024) "wasm demo activated")
  (data (i32.const 1100) "transport.serial.default.rx")
  (data (i32.const 1200) "{\"topic\":\"protocol.wasm.plugin\",\"payload\":{\"activated\":true}}")
  (data (i32.const 1400) "{\"id\":\"builtin.wasm-demo.chart\",\"title\":\"WASM Demo\",\"kind\":\"chart\",\"topic\":\"protocol.wasm.plugin\"}")

  (func (export "activate") (result i32)
    (call $log (i32.const 1024) (i32.const 19))
    (call $bus_subscribe (i32.const 1100) (i32.const 27))
    (call $bus_publish (i32.const 1200) (i32.const 61))
    (call $ui_panel_create (i32.const 1400) (i32.const 98))
    (i32.const 0))

  (func (export "on_event") (param $ptr i32) (param $len i32) (result i32)
    ;; 收到串口数据时发布响应事件
    (call $bus_publish (i32.const 1600) (i32.const 67))
    (i32.const 0))

  (func (export "deactivate") (result i32)
    (i32.const 0))
)
```

> **提示**：WAT 直接操作内存读写 JSON 字符串较繁琐。推荐复杂插件使用 Lua，简单高性能数据处理使用 WASM 解码器。

---

## 5. WASM 解码器开发

WASM 解码器专门用于将**二进制串口数据**解码为结构化 JSON。解码器订阅一个 input topic，输出到 output topic。

### 5.1 解码器清单 (decoder.json)

```json
{
  "id": "builtin.hex-byte",
  "name": "HEX Byte Decoder",
  "version": "0.1.0",
  "runtime": "wasm",
  "module": "decoder.wat",
  "function": "decode",
  "input_topic": "transport.serial.default.rx",
  "output_topic": "protocol.wasm.hex",
  "input_ptr": 0,
  "output_ptr": 32768,
  "output_cap": 1024
}
```

| 字段 | 说明 |
|------|------|
| `id` | 全局唯一标识符 |
| `name` | 显示名称 |
| `runtime` | 固定为 `"wasm"` |
| `module` | WAT/WASM 文件路径（相对于解码器目录） |
| `function` | 导出函数名（签名 `(ptr, len, out, cap: i32) -> i32`） |
| `input_topic` | 监听的 DataBus topic |
| `output_topic` | 解码结果的输出 topic |
| `input_ptr` | 输入数据写入的线性内存偏移 |
| `output_ptr` | 输出 JSON 应写入的线性内存偏移 |
| `output_cap` | 输出缓冲最大字节数 |

### 5.2 decode 函数签名

```wat
(func (export "decode") (param $ptr i32) (param $len i32) (param $out i32) (param $cap i32) (result i32)
  ;; $ptr: 输入数据起始地址
  ;; $len: 输入数据长度
  ;; $out: 输出缓冲起始地址
  ;; $cap: 输出缓冲最大容量
  ;; 返回值: 输出的 JSON 字节数（0 = 无输出，负数 = 错误码）
)
```

### 5.3 完整示例：单字节 HEX 解码器

```wat
(module
  (memory (export "memory") 1)

  ;; 辅助函数：nibble → hex char
  (func $hex (param $n i32) (result i32)
    (if (result i32)
      (i32.lt_u (local.get $n) (i32.const 10))
      (then (i32.add (local.get $n) (i32.const 48)))  ;; '0'-'9'
      (else (i32.add (i32.sub (local.get $n) (i32.const 10)) (i32.const 65)))  ;; 'A'-'F'
    ))

  (func (export "decode") (param $ptr i32) (param $len i32) (param $out i32) (param $cap i32) (result i32)
    ;; 至少需要 12 字节输出: {"hex":"XX"}
    (if (i32.lt_u (local.get $cap) (i32.const 12))
      (then (return (i32.const 0))))

    (local $b i32)
    (local.set $b (i32.load8_u (local.get $ptr)))

    ;; 写入 JSON: {"hex":"XX"}
    (i32.store8 (local.get $out) (i32.const 123))     ;; {
    (i32.store8 (i32.add (local.get $out) (i32.const 1)) (i32.const 34))  ;; "
    (i32.store8 (i32.add (local.get $out) (i32.const 2)) (i32.const 104)) ;; h
    (i32.store8 (i32.add (local.get $out) (i32.const 3)) (i32.const 101)) ;; e
    (i32.store8 (i32.add (local.get $out) (i32.const 4)) (i32.const 120)) ;; x
    (i32.store8 (i32.add (local.get $out) (i32.const 5)) (i32.const 34))  ;; "
    (i32.store8 (i32.add (local.get $out) (i32.const 6)) (i32.const 58))  ;; :
    (i32.store8 (i32.add (local.get $out) (i32.const 7)) (i32.const 34))  ;; "
    (i32.store8 (i32.add (local.get $out) (i32.const 8))
      (call $hex (i32.shr_u (local.get $b) (i32.const 4))))  ;; 高 nibble
    (i32.store8 (i32.add (local.get $out) (i32.const 9))
      (call $hex (i32.and (local.get $b) (i32.const 15))))    ;; 低 nibble
    (i32.store8 (i32.add (local.get $out) (i32.const 10)) (i32.const 34)) ;; "
    (i32.store8 (i32.add (local.get $out) (i32.const 11)) (i32.const 125));; }
    (i32.const 12))  ;; 返回 12 字节
)
```

---

## 6. Python 工具开发

Python 工具以子进程方式运行，通过 stdin/stdout 与系统交互。

### 6.1 输入格式

Python 脚本从 stdin 读取一行 JSON 任务描述：

```json
{
  "id": "pid-analysis",
  "tool": "pid-analyzer",
  "params": {
    "log_path": "logs/session.jsonl",
    "output_path": "reports/pid-analysis.json",
    "settling_tolerance_percent": 2.0
  },
  "timeout_ms": 30000
}
```

### 6.2 输出格式

向 stdout 输出 JSONL（每行一个 JSON 对象）：

```jsonl
{"type":"progress","message":"loading log","percent":0.1}
{"type":"progress","message":"analyzing samples","percent":0.5}
{"type":"result","result":{"sample_count":3,"metrics":{"overshoot_percent":20.0}}}
{"type":"error","message":"file not found"}
```

| type | 说明 |
|------|------|
| `progress` | 进度更新，`percent` 可选 (0.0~1.0) |
| `result` | 最终结果，`result` 为任意 JSON |
| `error` | 错误信息，`message` 为错误文本 |

### 6.3 完整示例：PID 分析器

```python
# pid_analyzer.py
import json, sys

def analyze(log_path, output_path, tolerance):
    # 读取 JSONL 日志
    samples = []
    with open(log_path) as f:
        for line in f:
            event = json.loads(line.strip())
            if event.get("topic") == "protocol.pid.sample":
                value = event["payload"].get("value", {})
                if isinstance(value, dict):
                    samples.append(value)

    # 进度报告
    print(json.dumps({"type": "progress", "message": f"loaded {len(samples)} samples", "percent": 0.3}),
          flush=True)

    # 计算指标
    if samples:
        targets = [s["target"] for s in samples]
        actuals = [s["actual"] for s in samples]
        errors = [abs(a - t) for a, t in zip(actuals, targets)]
        overshoot = max(0, max(actuals) - max(targets)) / max(targets) * 100

        result = {
            "sample_count": len(samples),
            "metrics": {
                "overshoot_percent": round(overshoot, 2),
                "mean_absolute_error": round(sum(errors) / len(errors), 4)
            }
        }
    else:
        result = {"sample_count": 0, "metrics": {}}

    # 保存报告
    with open(output_path, "w") as f:
        json.dump(result, f, indent=2)

    # 输出最终结果
    print(json.dumps({"type": "result", "result": result}), flush=True)

if __name__ == "__main__":
    task = json.loads(sys.stdin.readline())
    analyze(
        log_path=task["params"]["log_path"],
        output_path=task["params"]["output_path"],
        tolerance=task["params"].get("settling_tolerance_percent", 2.0)
    )
```

---

## 7. DataBus 与 Topic 规范

### 7.1 内置 Topic

| Topic | 方向 | Payload | 说明 |
|-------|------|---------|------|
| `transport.serial.default.rx` | Rx | Bytes | 串口接收数据 |
| `transport.serial.default.tx` | Tx | Bytes | 串口发送数据 |
| `log.system` | Internal | Text | 系统日志 |
| `protocol.pid.sample` | Internal | Json | PID 采样数据 |
| `protocol.imu.attitude` | Internal | Json | IMU 姿态数据 |
| `protocol.wasm.decoded` | Internal | Json | WASM 解码器输出 |
| `ui.panel.create` | Internal | Json | 动态创建面板 |
| `ui.panel.remove` | Internal | Json/Text | 动态移除面板 |
| `ui.form.changed` | Internal | Json | 表单数据变更 |
| `test.result` | Internal | Json | 测试报告 |

### 7.2 Topic 命名约定

```
<domain>.<subsystem>.<detail>

# 示例
protocol.pid.sample          # 协议层：PID 采样
protocol.imu.attitude        # 协议层：IMU 姿态
transport.serial.default.rx  # 传输层：串口接收
plugin.my-plugin.output      # 插件层：自定义 topic
```

### 7.3 Event 结构

```rust
struct Event {
    id: u64,              // 全局自增 ID
    timestamp_ms: u64,    // Unix 毫秒时间戳
    topic: String,        // 路由 topic
    source: String,       // 事件来源
    direction: Direction, // Rx / Tx / Internal
    payload: Payload,     // Empty / Bytes / Text / Json
    metadata: Value,      // JSON 元数据
}
```

### 7.4 Payload 格式

```json
// Bytes
{"kind": "bytes", "value": [65, 84, 13, 10]}

// Text
{"kind": "text", "value": "AT\r\n"}

// Json
{"kind": "json", "value": {"target": 50, "actual": 43}}

// Empty
{"kind": "empty", "value": null}
```

---

## 8. 测试框架

Lua 插件内建轻量级测试框架，通过 `test` 全局对象使用。

### 8.1 API

```lua
-- 设置超时
test.timeout(5000)

-- 定义 before/after 钩子
test.before_each(function()
  ctx.serial.open("COM3")
end)

test.after_each(function()
  ctx.serial.close()
end)

-- 日志（关联到当前测试用例）
test.log("starting test")

-- 断言
test.assert(1 + 1 == 2, "math is broken")

-- 等待事件
local event = test.expect("transport.serial.default.rx", 5000)

-- 定义测试用例
test.case("serial responds to AT", function()
  ctx.serial.send("AT\r\n")
  local line = ctx.serial.expect("OK", 2000)
  test.assert(line ~= nil, "no response")
  test.assert(string.find(line, "OK"), "unexpected response")
end)

test.case("PID values are reasonable", function()
  local event = test.expect("protocol.pid.sample", 5000)
  test.assert(event ~= nil, "no PID event received")
  -- event.payload 包含: { t, target, actual, output }
end)
```

### 8.2 测试报告结构

每个 `test.case` 执行后自动发布 `TestRunReport` 到 `test.result` topic：

```json
{
  "run_id": "test.lua-1717765200000",
  "source": "plugin:builtin.pid-tuner",
  "script_name": "test.lua",
  "started_ms": 1717765200000,
  "finished_ms": 1717765205123,
  "cases": [
    {
      "name": "serial responds to AT",
      "status": "passed",
      "duration_ms": 42,
      "logs": ["starting test"],
      "assertions": 2,
      "error": null,
      "raw_packets": [
        {
          "id": 42,
          "timestamp_ms": 1717765200100,
          "topic": "transport.serial.default.rx",
          "direction": "rx",
          "payload_text": "OK\r\n",
          "payload_hex": "4F 4B 0D 0A"
        }
      ]
    }
  ]
}
```

### 8.3 在 UI 中查看报告

测试报告在「测试」面板中显示，支持：
- 按 run_id 分组的测试运行
- 每个用例的通过/失败状态
- 断言计数和耗时
- 关联的原始数据包（前 8 条）
- 导出为 JSON 文件

---

## 9. 插件清单参考

### 9.1 plugin.json（Lua / WASM 插件）

```json
{
  "id": "my-company.my-plugin",
  "name": "My Plugin",
  "version": "1.0.0",
  "runtime": "lua",
  "main": "main.lua",
  "permissions": ["bus", "log", "serial", "ui", "storage", "timer", "testing"],
  "contributes": {
    "commands": [
      { "id": "my-plugin.do-something", "title": "Do Something" }
    ],
    "panels": [
      { "id": "my-plugin.chart", "title": "My Chart", "kind": "chart" },
      { "id": "my-plugin.form", "title": "My Form", "kind": "form" },
      { "id": "my-plugin.attitude", "title": "My 3D View", "kind": "attitude" }
    ],
    "settings": [
      { "id": "my-plugin.topic", "title": "Topic", "default": "protocol.my.topic" }
    ],
    "subscriptions": [
      { "topic": "transport.serial.default.rx" },
      { "topic": "protocol.pid.sample" }
    ]
  }
}
```

| 字段 | 必需 | 说明 |
|------|------|------|
| `id` | ✅ | 全局唯一标识符（推荐 `namespace.name` 格式） |
| `name` | ✅ | 显示名称 |
| `version` | ✅ | 语义化版本号 |
| `runtime` | ✅ | `"lua"` 或 `"wasm"` |
| `main` | ✅ | 入口文件（相对路径） |
| `permissions` | 否 | 权限列表，默认空 |
| `contributes` | 否 | 扩展点注册 |

#### 面板类型 (kind)

| kind | 说明 | 数据格式 |
|------|------|---------|
| `chart` | 折线图 | JSON `{t, 任意数字字段...}` 或 `key=value,key=value` 文本 |
| `form` | 参数表单 | 由用户交互 |
| `attitude` | 3D 姿态显示 | JSON `{roll, pitch, yaw}` 或 `roll=1.5,pitch=-2,yaw=90` 文本 |

### 9.2 decoder.json（WASM 解码器）

```json
{
  "id": "my-company.my-decoder",
  "name": "My Protocol Decoder",
  "version": "1.0.0",
  "runtime": "wasm",
  "module": "decoder.wasm",
  "function": "decode",
  "input_topic": "transport.serial.default.rx",
  "output_topic": "protocol.my.custom",
  "input_ptr": 0,
  "output_ptr": 32768,
  "output_cap": 4096
}
```

---

## 10. 权限系统

每个权限对应一组 API 访问能力：

| 权限 | Lua API | WASM Host 函数 | 说明 |
|------|---------|---------------|------|
| `bus` | `ctx.bus.*` | `host.bus_publish`, `host.bus_subscribe` | 发布/订阅事件 |
| `log` | `ctx.log.*` | `host.log` | 写入系统日志 |
| `serial` | `ctx.serial.*` | — | 串口读写 |
| `ui` | `ctx.ui.*` | `host.ui_panel_create` | 创建动态面板 |
| `storage` | — | `host.storage_set`, `host.storage_get` | 持久化键值存储 |
| `timer` | 预留 | — | 定时器（规划中） |
| `testing` | 内置可用 | — | 测试框架 |

权限在 `plugin.json` 的 `permissions` 数组中声明。运行时由 `PermissionManager` 检查，未声明的权限调用会返回 `PermissionDenied` 错误。

默认允许的权限白名单（`PermissionManager::default()`）：
```rust
["bus", "log", "serial", "ui", "storage", "timer", "testing"]
```

---

## 附录 A：快捷键参考

| 快捷键 | 功能 |
|--------|------|
| `Ctrl+1` | 设备面板 |
| `Ctrl+2` | 脚本面板 |
| `Ctrl+3` | 插件面板 |
| `Ctrl+4` | Python 工具 |
| `Ctrl+5` | 设置面板 |
| `Ctrl+B` | 切换底部终端区 |
| `Ctrl+I` | 切换检查器面板 |
| `Ctrl+W` | 关闭当前动态标签 |
| `Ctrl+R` | 刷新串口列表 |
| `Ctrl+Shift+O` | 打开串口 |
| `Ctrl+Enter` | 发送（发送区） |

## 附录 B：Event 的 metadata 约定

系统组件在发布事件时附带标准 metadata：

| metadata key | 说明 | 示例 |
|-------------|------|------|
| `replay` | 回放事件标记 | `true` |
| `original_source` | 回放前的原始 source | `"serial:COM3"` |
| `level` | 日志级别 | `"info"`, `"warn"`, `"error"` |
| `decoder_id` | WASM 解码器 ID | `"builtin.hex-byte"` |
| `input_event_id` | 触发解码的原始事件 ID | `42` |

## 附录 C：项目 workspace 结构

```
Cargo.toml (workspace)
├── crates/
│   ├── app/          # egui UI 主程序
│   ├── core/         # Event/Payload/Direction/LogLevel 基础类型
│   ├── databus/      # 发布/订阅事件总线
│   ├── transport/    # 串口管理与 HEX 解析
│   ├── recorder/     # JSONL 录制与回放引擎
│   ├── lua_host/     # Lua 5.4 运行时（mlua）
│   ├── wasm_host/    # WASM 解码器 + 插件运行时（wasmtime）
│   ├── python_host/  # Python 子进程工具运行时
│   ├── extension/    # 插件管理器（发现/启用/权限/进程）
│   ├── testing/      # 测试报告存储
│   └── panels/       # 所有 UI 面板（终端/图表/回放/脚本/插件等）
├── plugins/          # 插件仓库
├── wasm-decoders/    # WASM 解码器仓库
├── tools/            # Python 工具仓库
├── scripts/          # 用户 Lua 脚本
└── logs/             # JSONL 会话日志
```

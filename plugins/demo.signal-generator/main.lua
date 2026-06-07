-- 串口信号发生器 Demo
-- 插件负责：定时生成信号 → COM3 发出
-- 用户负责：设备面板打开 COM2 接收
-- COM2↔COM3 需通过虚拟串口软件互联

local t = 0

-- 图表（显示从串口 RX 解析到的数据）
ctx.ui.create_chart({
    id = "demo-signal-chart",
    title = "串口信号波形",
    topic_prefix = "protocol.demo."
})

-- 参数面板
ctx.ui.create_form({
    id = "demo-signal-form",
    title = "参数",
    fields = {
        { id = "amplitude",  label = "振幅",        kind = "number", default = 10.0 },
        { id = "frequency",  label = "频率 Hz",     kind = "number", default = 1.0  },
        { id = "interval",   label = "发送间隔 ms",  kind = "number", default = 200  },
        { id = "tx_port",    label = "发送端口",     kind = "text",   default = "COM3" },
    }
})

-- 监听串口 RX（由设备面板打开的 COM2 接收）
ctx.bus.on("transport.serial.default.rx", function(event)
    local text = tostring(event.payload)
    local t_v = text:match('"t":(%d+)')
    local g_v = text:match('"target":([%d.]+)')
    local a_v = text:match('"actual":([%d.-]+)')
    local o_v = text:match('"output":([%d.-]+)')
    if t_v and g_v and a_v and o_v then
        ctx.bus.publish("protocol.demo.sample", {
            t = tonumber(t_v), target = tonumber(g_v),
            actual = tonumber(a_v), output = tonumber(o_v),
        })
    end
end)

-- 打开发送端口
local tx_port = ctx.storage.get("tx_port") or "COM3"
pcall(function()
    ctx.serial.open({ port_name = tx_port, baud_rate = 115200 })
    ctx.log.info("发送端口已打开: " .. tx_port)
end)

-- 定时发送正弦波数据
local timer_id = ctx.timer.every(200, function()
    t = t + 1
    local amp      = tonumber(ctx.storage.get("amplitude")) or 10.0
    local freq     = tonumber(ctx.storage.get("frequency")) or 1.0
    local interval = tonumber(ctx.storage.get("interval")) or 200
    local target   = 50.0
    local phase    = (t * interval / 1000.0) * freq * 2.0 * math.pi
    local actual   = target + math.sin(phase) * amp
    local output   = math.sin(phase) * 0.8

    -- 直接推送图表（始终有效）
    ctx.bus.publish("protocol.demo.sample", { t = t, target = target, actual = actual, output = output })

    -- 同时通过串口发出（COM2 手动接收后，RX 监听器也会推图表）
    local msg = string.format('{"t":%d,"target":%.1f,"actual":%.2f,"output":%.3f}\n',
        t, target, actual, output)
    local port = ctx.storage.get("tx_port") or "COM3"
    pcall(function() ctx.serial.send_to(port, msg) end)
end)

-- 参数变更回调
ctx.bus.on("ui.form.changed", function(event)
    local v = event.payload and event.payload.values
    if v then for k, val in pairs(v) do ctx.storage.set(k, tostring(val)) end end
end)

ctx.log.info("就绪: 发送 → " .. (ctx.storage.get("tx_port") or "COM3"))

on_disable(function()
    ctx.timer.cancel(timer_id)
    pcall(function() ctx.serial.close_port(ctx.storage.get("tx_port") or "COM3") end)
    ctx.ui.remove_panel("demo-signal-chart")
    ctx.ui.remove_panel("demo-signal-form")
    ctx.log.info("已停止, 共 " .. t .. " 样本")
end)

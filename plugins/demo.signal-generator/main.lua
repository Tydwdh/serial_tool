-- 信号发生器 Demo
-- 测试：图表渲染、串口收发、多端口

local t = 0
local sample_count = 0
local tx_open = false

-- 获取可用串口列表
local function port_list()
    local ports = {}
    pcall(function()
        for _, p in ipairs(ctx.serial.list()) do
            table.insert(ports, { id = p.port_name, label = p.port_name .. " (" .. p.port_type .. ")" })
        end
    end)
    if #ports == 0 then
        ports = { { id = "COM1", label = "COM1" }, { id = "COM2", label = "COM2" }, { id = "COM3", label = "COM3" } }
    end
    return ports
end

local ports = port_list()

-- 图表面板
ctx.ui.create_chart({
    id = "demo-signal-chart",
    title = "信号波形",
    topic_prefix = "protocol.demo."
})

-- 参数面板
ctx.ui.create_form({
    id = "demo-signal-form",
    title = "信号参数",
    fields = {
        { id = "mode",       label = "模式",        kind = "text", default = "direct" },
        { id = "freq_hz",    label = "频率(Hz)",    kind = "text", default = "2" },
        { id = "amplitude",  label = "振幅",        kind = "text", default = "10" },
        { id = "tx_port",    label = "发送端口",    kind = "text", default = "COM3" },
        { id = "baud_rate",  label = "波特率",      kind = "text", default = "115200" },
    }
})

-- 监听串口 RX
ctx.bus.on("transport.serial.default.rx", function(event)
    local text = tostring(event.payload)
    local t_v = text:match('"t":(%d+)')
    local a_v = text:match('"actual":([%d.-]+)')
    local g_v = text:match('"target":([%d.]+)')
    local o_v = text:match('"output":([%d.-]+)')
    if t_v and g_v and a_v and o_v then
        ctx.bus.publish("protocol.demo.sample", {
            t = tonumber(t_v), target = tonumber(g_v),
            actual = tonumber(a_v), output = tonumber(o_v),
        })
        sample_count = sample_count + 1
    end
end)

-- 打开发送端口
local function ensure_tx_port()
    if tx_open then return true end
    local port = ctx.storage.get("tx_port") or "COM3"
    local baud = ctx.storage.get("baud_rate") or "115200"
    local ok, err = pcall(function()
        ctx.serial.open({ port_name = port, baud_rate = tonumber(baud) or 115200 })
    end)
    if ok then
        tx_open = true
        ctx.log.info("已打开发送端口 " .. port)
        return true
    else
        ctx.log.warn("无法打开 " .. port .. ": " .. tostring(err))
        return false
    end
end

-- 主循环
local timer_id = ctx.timer.every(100, function()
    t = t + 1
    local mode  = ctx.storage.get("mode") or "direct"
    local freq  = tonumber(ctx.storage.get("freq_hz")) or 2
    local amp   = tonumber(ctx.storage.get("amplitude")) or 10
    local target = 50.0
    local phase  = (t / (1000.0 / 100.0)) * freq * 2.0 * math.pi
    local actual = target + math.sin(phase) * amp
    local output = math.sin(phase) * 0.8
    local msg = string.format('{"t":%d,"target":%.1f,"actual":%.2f,"output":%.3f}\n',
        t, target, actual, output)

    if mode == "serial" then
        if ensure_tx_port() then
            pcall(function() ctx.serial.send_to(ctx.storage.get("tx_port") or "COM3", msg) end)
        end
    else
        ctx.bus.publish("protocol.demo.sample", {
            t = t, target = target, actual = actual, output = output,
        })
    end
end)

-- 参数变更回调
ctx.bus.on("ui.form.changed", function(event)
    local v = event.payload and event.payload.values
    if v then
        for k, val in pairs(v) do ctx.storage.set(k, tostring(val)) end
    end
end)

ctx.log.info("就绪: 模式=" .. (ctx.storage.get("mode") or "direct") ..
    " 频率=" .. (ctx.storage.get("freq_hz") or "2") .. "Hz")

on_disable(function()
    ctx.timer.cancel(timer_id)
    if tx_open then pcall(function() ctx.serial.close_port(ctx.storage.get("tx_port") or "COM3") end) end
    ctx.ui.remove_panel("demo-signal-chart")
    ctx.ui.remove_panel("demo-signal-form")
    ctx.log.info("已停止, 共 " .. sample_count .. " 样本")
end)

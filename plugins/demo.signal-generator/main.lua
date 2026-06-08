-- 串口信号发生器 Demo
--
-- 默认模式：
--   chart_mode = direct
--   插件直接发布 protocol.demo.sample，图表立即有数据。
--
-- 串口回环模式：
--   chart_mode = serial_loopback
--   插件只向 tx_port 发送 JSON。
--   用户需要用虚拟串口把 tx_port 和接收端口互联。
--   接收端收到 RX 后，插件再解析 RX 并发布 protocol.demo.sample。
--
-- 这样避免 direct publish 和 RX publish 同时发生导致图表重复。

local t = 0

local function storage_get(key, fallback)
    local value = ctx.storage.get(key)
    if value == nil or value == "" then
        return fallback
    end
    return value
end

local function number_setting(key, fallback)
    local value = tonumber(ctx.storage.get(key))
    if value == nil then
        return fallback
    end
    return value
end

local function chart_mode()
    return storage_get("chart_mode", "direct")
end

local function tx_port()
    return storage_get("tx_port", "COM3")
end

local function publish_sample(sample)
    ctx.bus.publish("protocol.demo.sample", sample)
end

local function parse_sample_text(text)
    local t_v = text:match('"t":(%d+)')
    local g_v = text:match('"target":([%d.]+)')
    local a_v = text:match('"actual":([%d.-]+)')
    local o_v = text:match('"output":([%d.-]+)')

    if not (t_v and g_v and a_v and o_v) then
        return nil
    end

    return {
        t = tonumber(t_v),
        target = tonumber(g_v),
        actual = tonumber(a_v),
        output = tonumber(o_v),
    }
end

ctx.ui.create_chart({
    id = "demo-signal-chart",
    title = "串口信号波形",
    topic_prefix = "protocol.demo."
})

ctx.ui.create_form({
    id = "demo-signal-form",
    title = "参数",
    fields = {
        { id = "amplitude",  label = "振幅",        kind = "number", default = 10.0 },
        { id = "frequency",  label = "频率 Hz",     kind = "number", default = 1.0  },
        { id = "interval",   label = "发送间隔 ms",  kind = "number", default = 200  },
        { id = "tx_port",    label = "发送端口",     kind = "text",   default = "COM3" },
        { id = "chart_mode", label = "图表模式",     kind = "text",   default = "direct" },
    }
})

ctx.bus.on("transport.serial.default.rx", function(event)
    if chart_mode() ~= "serial_loopback" then
        return
    end

    local text = tostring(event.payload)
    local sample = parse_sample_text(text)

    if sample then
        publish_sample(sample)
    end
end)

pcall(function()
    ctx.serial.open({
        port_name = tx_port(),
        baud_rate = 115200
    })

    ctx.log.info("发送端口已打开: " .. tx_port())
end)

local timer_id = ctx.timer.every(200, function()
    t = t + 1

    local amp = number_setting("amplitude", 10.0)
    local freq = number_setting("frequency", 1.0)
    local interval = number_setting("interval", 200)

    local target = 50.0
    local phase = (t * interval / 1000.0) * freq * 2.0 * math.pi
    local actual = target + math.sin(phase) * amp
    local output = math.sin(phase) * 0.8

    local sample = {
        t = t,
        target = target,
        actual = actual,
        output = output,
    }

    if chart_mode() == "direct" then
        publish_sample(sample)
    end

    local msg = string.format(
        '{"t":%d,"target":%.1f,"actual":%.2f,"output":%.3f}\n',
        t,
        target,
        actual,
        output
    )

    pcall(function()
        ctx.serial.send_to(tx_port(), msg)
    end)
end)

ctx.bus.on("ui.form.changed", function(event)
    local values = event.payload and event.payload.values

    if values then
        for key, value in pairs(values) do
            ctx.storage.set(key, tostring(value))
        end
    end
end)

ctx.log.info("就绪: 发送 → " .. tx_port() .. "，图表模式 → " .. chart_mode())

on_disable(function()
    ctx.timer.cancel(timer_id)

    pcall(function()
        ctx.serial.close_port(tx_port())
    end)

    ctx.ui.remove_panel("demo-signal-chart")
    ctx.ui.remove_panel("demo-signal-form")

    ctx.log.info("已停止, 共 " .. t .. " 样本")
end)
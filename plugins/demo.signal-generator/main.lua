-- 串口链路压力测试 Demo
--
-- 目标链路：
--   插件打开 TX 端口，例如 COM3
--   COM3 <-> COM2 由虚拟串口软件连接
--   用户在设备页打开 RX 端口，例如 COM2
--   插件监听 transport.serial.default.rx
--   收到 RX 后按行解析 JSON
--   根据 seq 检测丢包、重复、乱序
--   发布 protocol.demo.sample 给图表
--
-- 重要：
--   图表只显示“真正从 RX 收到并解析成功”的数据。
--   插件不会绕过串口链路直接推图表。
--
-- 建议：
--   行尾选择 LF 或 CRLF。
--   如果选择“无行尾”，高频下串口读包可能粘包/拆包，丢包检测会不可靠。

local seq = 0
local sent = 0
local received = 0
local parse_errors = 0

local lost_total = 0
local duplicate_total = 0
local out_of_order_total = 0
local last_rx_seq = nil

local opened_port = nil
local opened_baud = nil

local last_send_ms = ctx.now_ms()
local send_accum = 0.0
local last_error = nil

local rx_buffers = {}

local function storage_get(key, fallback)
    local value = ctx.storage.get(key)
    if value == nil or value == "" then
        return fallback
    end
    return value
end

local function storage_number(key, fallback)
    local value = tonumber(ctx.storage.get(key))
    if value == nil then
        return fallback
    end
    return value
end

local function storage_bool(key, fallback)
    local value = ctx.storage.get(key)
    if value == nil or value == "" then
        return fallback
    end

    value = tostring(value):lower()
    return value == "true" or value == "1" or value == "yes" or value == "on"
end

local function serial_port_options()
    local result = {}
    local ok, ports = pcall(function()
        return ctx.serial.list()
    end)

    if ok and ports then
        for _, port in ipairs(ports) do
            local name = tostring(port.port_name or "")
            local kind = tostring(port.port_type or "")
            if name ~= "" then
                local label = name
                if kind ~= "" then
                    label = label .. "  " .. kind
                end

                table.insert(result, {
                    label = label,
                    value = name
                })
            end
        end
    end

    local has_com3 = false
    for _, option in ipairs(result) do
        if option.value == "COM3" then
            has_com3 = true
            break
        end
    end

    if not has_com3 then
        table.insert(result, {
            label = "COM3",
            value = "COM3"
        })
    end

    if #result == 0 then
        table.insert(result, {
            label = "COM3",
            value = "COM3"
        })
    end

    return result
end

local function rx_port_options()
    local result = {
        {
            label = "任意 RX 端口",
            value = "__any__"
        }
    }

    for _, option in ipairs(serial_port_options()) do
        table.insert(result, option)
    end

    return result
end

local function tx_port()
    return storage_get("tx_port", "COM3")
end

local function rx_port()
    return storage_get("rx_port", "__any__")
end

local function baud_rate()
    return math.floor(storage_number("baud_rate", 115200))
end

local function rate_hz()
    return math.max(1, storage_number("rate_hz", 50))
end

local function max_burst()
    return math.max(1, math.floor(storage_number("max_burst", 50)))
end

local function amplitude()
    return storage_number("amplitude", 10.0)
end

local function waveform()
    return storage_get("waveform", "sine")
end

local function line_ending()
    return storage_get("line_ending", "lf")
end

local function enabled()
    return storage_bool("enabled", false)
end

local function close_tx()
    if opened_port ~= nil then
        local port = opened_port

        pcall(function()
            ctx.serial.close_port(port)
        end)

        ctx.log.info("TX 端口已关闭: " .. port)

        opened_port = nil
        opened_baud = nil
    end
end

local function reset_counters()
    seq = 0
    sent = 0
    received = 0
    parse_errors = 0
    lost_total = 0
    duplicate_total = 0
    out_of_order_total = 0
    last_rx_seq = nil
    rx_buffers = {}
    send_accum = 0.0
    last_send_ms = ctx.now_ms()
end

local function ensure_tx_open()
    local desired_port = tx_port()
    local desired_baud = baud_rate()

    if opened_port == desired_port and opened_baud == desired_baud then
        return true
    end

    close_tx()

    local ok, err = pcall(function()
        ctx.serial.open({
            port_name = desired_port,
            baud_rate = desired_baud
        })
    end)

    if ok then
        opened_port = desired_port
        opened_baud = desired_baud
        last_error = nil
        ctx.log.info("TX 端口已打开: " .. desired_port .. " @ " .. tostring(desired_baud))
        return true
    end

    local text = tostring(err)
    if last_error ~= text then
        ctx.log.warn("TX 端口打开失败: " .. text)
        last_error = text
    end

    return false
end

local function make_actual_value()
    local amp = amplitude()
    local target = 50.0
    local mode = waveform()

    if mode == "square" then
        if seq % 2 == 0 then
            return target + amp
        else
            return target - amp
        end
    elseif mode == "saw" then
        local phase = seq % 100
        return target - amp + (phase / 99.0) * amp * 2.0
    elseif mode == "counter" then
        return seq % 100
    else
        local hz = math.max(0.1, storage_number("signal_hz", 1.0))
        local phase = (seq / math.max(rate_hz(), 1)) * hz * 2.0 * math.pi
        return target + math.sin(phase) * amp
    end
end

local function build_packet()
    seq = seq + 1

    local target = 50.0
    local actual = make_actual_value()
    local output = (actual - target) / math.max(amplitude(), 1.0)

    local msg = string.format(
        '{"seq":%d,"target":%.1f,"actual":%.3f,"output":%.4f}',
        seq,
        target,
        actual,
        output
    )

    local ending = line_ending()

    if ending == "lf" then
        msg = msg .. "\n"
    elseif ending == "crlf" then
        msg = msg .. "\r\n"
    end

    return msg
end

local function send_one()
    if not ensure_tx_open() then
        return
    end

    local msg = build_packet()

    local ok, err = pcall(function()
        ctx.serial.send_to(tx_port(), msg)
    end)

    if ok then
        sent = sent + 1
        return
    end

    local text = tostring(err)
    if last_error ~= text then
        ctx.log.warn("发送失败: " .. text)
        last_error = text
    end
end

local function parse_packet(text)
    local seq_v = text:match('"seq":(%d+)')
    local target_v = text:match('"target":([%d.-]+)')
    local actual_v = text:match('"actual":([%d.-]+)')
    local output_v = text:match('"output":([%d.-]+)')

    if not (seq_v and target_v and actual_v and output_v) then
        return nil
    end

    return {
        seq = tonumber(seq_v),
        target = tonumber(target_v),
        actual = tonumber(actual_v),
        output = tonumber(output_v)
    }
end

local function update_loss_counters(rx_seq)
    if last_rx_seq == nil then
        last_rx_seq = rx_seq
        return
    end

    if rx_seq == last_rx_seq then
        duplicate_total = duplicate_total + 1
        return
    end

    if rx_seq < last_rx_seq then
        out_of_order_total = out_of_order_total + 1
        return
    end

    local expected = last_rx_seq + 1

    if rx_seq > expected then
        lost_total = lost_total + (rx_seq - expected)
    end

    last_rx_seq = rx_seq
end

local function publish_rx_sample(sample)
    received = received + 1
    update_loss_counters(sample.seq)

    local expected_total = received + lost_total
    local loss_rate_percent = 0.0

    if expected_total > 0 then
        loss_rate_percent = lost_total * 100.0 / expected_total
    end

    ctx.bus.publish("protocol.demo.sample", {
        t = sample.seq,

        target = sample.target,
        actual = sample.actual,
        output = sample.output,

        sent_total = sent,
        rx_total = received,
        lost_total = lost_total,
        duplicate_total = duplicate_total,
        out_of_order_total = out_of_order_total,
        parse_errors = parse_errors,
        loss_rate_percent = loss_rate_percent
    })
end

local function handle_line(line)
    if line == nil or line == "" then
        return
    end

    line = line:gsub("\r", "")

    local sample = parse_packet(line)

    if sample == nil then
        parse_errors = parse_errors + 1
        return
    end

    publish_rx_sample(sample)
end

local function feed_rx_text(port, text)
    local ending = line_ending()

    if ending == "none" then
        -- 无行尾时没有可靠帧边界，只能尝试按整段解析。
        -- 高频压测建议不要用这个模式。
        handle_line(text)
        return
    end

    local current = rx_buffers[port] or ""
    current = current .. text

    while true do
        local start_index, end_index = current:find("\n", 1, true)
        if start_index == nil then
            break
        end

        local line = current:sub(1, start_index - 1)
        current = current:sub(end_index + 1)

        handle_line(line)
    end

    -- 防止异常情况下 buffer 无限增长。
    if #current > 4096 then
        parse_errors = parse_errors + 1
        current = ""
    end

    rx_buffers[port] = current
end

local function maybe_handle_rx_event(event)
    local selected_rx_port = rx_port()
    local event_port = ""

    if event.metadata and event.metadata.port then
        event_port = tostring(event.metadata.port)
    end

    if selected_rx_port ~= "__any__" and event_port ~= selected_rx_port then
        return
    end

    feed_rx_text(event_port, tostring(event.payload))
end

ctx.ui.create_chart({
    id = "demo-signal-chart",
    title = "串口链路 RX 解析结果",
    topic_prefix = "protocol.demo.sample"
})

ctx.ui.create_form({
    id = "demo-signal-form",
    title = "串口链路压力测试",
    auto_apply = true,
    fields = {
        {
            id = "enabled",
            label = "发送启用",
            kind = "checkbox",
            default = false
        },
        {
            id = "reset_stats",
            label = "重置统计",
            kind = "checkbox",
            default = false
        },
        {
            id = "tx_port",
            label = "TX 发送端口",
            kind = "select",
            default = "COM3",
            options = serial_port_options()
        },
        {
            id = "rx_port",
            label = "RX 解析端口",
            kind = "select",
            default = "__any__",
            options = rx_port_options()
        },
        {
            id = "baud_rate",
            label = "波特率",
            kind = "select",
            default = "115200",
            options = {
                { label = "9600",   value = "9600" },
                { label = "19200",  value = "19200" },
                { label = "38400",  value = "38400" },
                { label = "57600",  value = "57600" },
                { label = "115200", value = "115200" },
                { label = "230400", value = "230400" },
                { label = "460800", value = "460800" },
                { label = "921600", value = "921600" }
            }
        },
        {
            id = "rate_hz",
            label = "发送频率",
            kind = "select",
            default = "50",
            options = {
                { label = "1 Hz",    value = "1" },
                { label = "5 Hz",    value = "5" },
                { label = "10 Hz",   value = "10" },
                { label = "20 Hz",   value = "20" },
                { label = "50 Hz",   value = "50" },
                { label = "100 Hz",  value = "100" },
                { label = "200 Hz",  value = "200" },
                { label = "500 Hz",  value = "500" },
                { label = "1000 Hz", value = "1000" }
            }
        },
        {
            id = "max_burst",
            label = "单次最大突发",
            kind = "select",
            default = "50",
            options = {
                { label = "10 包", value = "10" },
                { label = "50 包", value = "50" },
                { label = "100 包", value = "100" },
                { label = "500 包", value = "500" }
            }
        },
        {
            id = "waveform",
            label = "数据波形",
            kind = "select",
            default = "sine",
            options = {
                { label = "正弦", value = "sine" },
                { label = "方波", value = "square" },
                { label = "锯齿", value = "saw" },
                { label = "计数", value = "counter" }
            }
        },
        {
            id = "amplitude",
            label = "振幅",
            kind = "slider",
            default = 10.0,
            min = 1.0,
            max = 50.0,
            step = 1.0
        },
        {
            id = "signal_hz",
            label = "波形频率",
            kind = "select",
            default = "1",
            options = {
                { label = "0.5 Hz", value = "0.5" },
                { label = "1 Hz",   value = "1" },
                { label = "2 Hz",   value = "2" },
                { label = "5 Hz",   value = "5" },
                { label = "10 Hz",  value = "10" }
            }
        },
        {
            id = "line_ending",
            label = "行尾",
            kind = "select",
            default = "lf",
            options = {
                { label = "LF \\n", value = "lf" },
                { label = "CRLF \\r\\n", value = "crlf" },
                { label = "无", value = "none" }
            }
        }
    }
})

ctx.bus.on("ui.replay.reset", function()
    reset_counters()
end)

ctx.bus.on("transport.serial.default.rx", function(event)
    maybe_handle_rx_event(event)
end)

ctx.bus.on("ui.form.changed", function(event)
    if not event.payload or event.payload.panel_id ~= "demo-signal-form" then
        return
    end

    local values = event.payload.values

    if values then
        for key, value in pairs(values) do
            ctx.storage.set(key, tostring(value))
        end
    end

    if storage_bool("reset_stats", false) then
        reset_counters()
        ctx.storage.set("reset_stats", "false")
        ctx.log.info("统计已重置")
    end

    if not enabled() then
        close_tx()
        ctx.log.info("发送已暂停")
    else
        ensure_tx_open()
        ctx.log.info(
            "发送已启用: "
            .. tx_port()
            .. " @ "
            .. tostring(baud_rate())
            .. ", "
            .. tostring(rate_hz())
            .. " Hz"
        )
    end
end)

local timer_id = ctx.timer.every(10, function()
    if not enabled() then
        last_send_ms = ctx.now_ms()
        send_accum = 0.0
        return
    end

    local now = ctx.now_ms()
    local elapsed = now - last_send_ms

    if elapsed <= 0 then
        return
    end

    last_send_ms = now

    send_accum = send_accum + elapsed * rate_hz() / 1000.0

    local count = math.floor(send_accum)

    if count <= 0 then
        return
    end

    local burst = max_burst()
    if count > burst then
        count = burst
    end

    send_accum = send_accum - count

    for _ = 1, count do
        send_one()
    end
end)

ctx.log.info("串口链路压力测试插件已就绪。打开 COM2 接收端后，在参数面板启用发送。")

on_disable(function()
    ctx.timer.cancel(timer_id)
    close_tx()
    ctx.ui.remove_panel("demo-signal-chart")
    ctx.ui.remove_panel("demo-signal-form")
    ctx.log.info(
        "插件已停止: sent="
        .. tostring(sent)
        .. ", received="
        .. tostring(received)
        .. ", lost="
        .. tostring(lost_total)
        .. ", duplicate="
        .. tostring(duplicate_total)
        .. ", out_of_order="
        .. tostring(out_of_order_total)
        .. ", parse_errors="
        .. tostring(parse_errors)
    )
end)

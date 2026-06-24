-- 串口图表插件模板
--
-- 输入数据格式示例：
--   {"seq":1,"value":12.3,"target":50.0}
--
-- 实时链路：
--   transport.serial.default.rx
--     -> main.lua 解析
--     -> protocol.template.sample
--     -> 图表显示
--
-- 回放链路：
--   RawSerial 录制
--     -> replay.lua 解析
--     -> protocol.template.sample
--     -> 图表显示

local CHART_ID = "template.serial-chart.chart"
local FORM_ID = "template.serial-chart.form"
local TOPIC_OUT = "protocol.template.sample"

local selected_port = ctx.session.get("rx_port") or "__any__"
local parse_errors = 0
local received = 0

ctx.log.info("串口图表插件启动: " .. tostring(ctx.plugin.id))

ctx.ui.create_chart({
    id = CHART_ID,
    title = "串口数据图表",
    topic_prefix = "protocol.template."
})

ctx.ui.create_form({
    id = FORM_ID,
    title = "串口图表参数",
    auto_apply = true,
    fields = {
        {
            id = "rx_port",
            label = "RX 端口",
            kind = "text",
            default = selected_port
        },
        {
            id = "show_log",
            label = "打印解析日志",
            kind = "checkbox",
            default = false
        }
    }
})

local function parse_packet(text)
    local seq = tonumber(text:match('"seq":(%d+)'))
    local value = tonumber(text:match('"value":([%d.-]+)'))
    local target = tonumber(text:match('"target":([%d.-]+)'))

    if not seq or not value then
        return nil
    end

    return {
        seq = seq,
        value = value,
        target = target or 0.0
    }
end

local function handle_text(port, text)
    if selected_port ~= "__any__" and selected_port ~= "" and port ~= selected_port then
        return
    end

    local sample = parse_packet(text)

    if not sample then
        parse_errors = parse_errors + 1
        return
    end

    received = received + 1

    ctx.bus.publish(TOPIC_OUT, {
        t = sample.seq,
        value = sample.value,
        target = sample.target,
        received = received,
        parse_errors = parse_errors
    })

    if ctx.session.get("show_log") == "true" then
        ctx.log.info(string.format(
            "parsed port=%s seq=%d value=%.3f",
            tostring(port),
            sample.seq,
            sample.value
        ))
    end
end

ctx.bus.on("transport.serial.default.rx", function(event)
    local port = "default"

    if event.metadata and event.metadata.port then
        port = tostring(event.metadata.port)
    end

    handle_text(port, tostring(event.payload))
end)

ctx.bus.on("ui.form.changed", function(event)
    if not event.payload or event.payload.panel_id ~= FORM_ID then
        return
    end

    local values = event.payload.values or {}

    if values.rx_port ~= nil then
        selected_port = tostring(values.rx_port)
        ctx.session.set("rx_port", selected_port)
    end

    if values.show_log ~= nil then
        ctx.session.set("show_log", tostring(values.show_log))
    end

    ctx.log.info("串口图表参数已更新")
end)

on_disable(function()
    ctx.ui.remove_panel(CHART_ID)
    ctx.ui.remove_panel(FORM_ID)
    ctx.log.info(
        "串口图表插件停止: received="
        .. tostring(received)
        .. ", parse_errors="
        .. tostring(parse_errors)
    )
end)

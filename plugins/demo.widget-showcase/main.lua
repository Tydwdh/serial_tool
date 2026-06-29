-- Widget 展示插件：演示 Chart / Gauge / Attitude / Form 四种面板
--
-- Chart:   正弦波折线图（50Hz 周期，100ms update）
-- Gauge:   温度仪表（色区 0-60-80-100）+ 电压仪表（色区 3.0-3.3-3.6-5.0）
-- Attitude: 正弦波驱动的缓慢旋转
-- Form:    控制频率 / 振幅 / 温度偏移 / 电压偏移

local CHART_ID = "showcase.chart"
local GAUGE_TEMP_ID = "showcase.gauge-temp"
local GAUGE_VOLTAGE_ID = "showcase.gauge-voltage"
local ATTITUDE_ID = "showcase.attitude"
local FORM_ID = "showcase.form"

local freq = 1.0      -- Hz
local amplitude = 1.0
local temp_offset = 50.0
local voltage_offset = 3.3

local start_time = ctx.now_ms()
local seq = 0

ctx.log.info("Widget 展示插件启动")

-- ── 创建面板 ──

ctx.ui.create_chart({
    id = CHART_ID,
    title = "正弦波图表",
    topic = "widget.showcase.chart.sample",
    card = true
})

ctx.ui.create_gauge({
    id = GAUGE_TEMP_ID,
    title = "温度",
    topic = "widget.showcase.temperature",
    min = 0,
    max = 100,
    unit = "°C",
    label = "传感器温度",
    zones = {
        { from = 0,  to = 60, color = "green" },
        { from = 60, to = 80, color = "yellow" },
        { from = 80, to = 100, color = "red" }
    },
    card = true
})

ctx.ui.create_gauge({
    id = GAUGE_VOLTAGE_ID,
    title = "电压",
    topic = "widget.showcase.voltage",
    min = 0,
    max = 5,
    unit = "V",
    label = "输入电压",
    zones = {
        { from = 3.0, to = 3.6, color = "green" },
        { from = 2.5, to = 3.0, color = "yellow" },
        { from = 0,   to = 2.5, color = "red" },
        { from = 3.6, to = 5.0, color = "red" }
    },
    card = true
})

ctx.ui.create_attitude({
    id = ATTITUDE_ID,
    title = "姿态指示器",
    topic = "widget.showcase.attitude",
    card = true
})

ctx.ui.create_form({
    id = FORM_ID,
    title = "参数控制",
    auto_apply = true,
    card = true,
    fields = {
        {
            id = "freq",
            label = "频率 (Hz)",
            kind = "slider",
            min = 0.1, max = 5.0, step = 0.1,
            default = 1.0
        },
        {
            id = "amplitude",
            label = "振幅",
            kind = "slider",
            min = 0.1, max = 3.0, step = 0.1,
            default = 1.0
        },
        {
            id = "temp_offset",
            label = "温度偏移 (°C)",
            kind = "slider",
            min = 0, max = 100, step = 1,
            default = 50.0
        },
        {
            id = "voltage_offset",
            label = "电压偏移 (V)",
            kind = "slider",
            min = 0, max = 5, step = 0.1,
            default = 3.3
        },
        { kind = "separator" },
        {
            id = "status",
            label = "运行状态",
            kind = "status",
            default = { text = "运行中", level = "running" }
        },
        {
            id = "samples",
            label = "已生成样本",
            kind = "label",
            default = "0"
        }
    }
})

-- ── 参数变更 ──

ctx.bus.on("ui.form.changed", function(event)
    if not event.payload or event.payload.panel_id ~= FORM_ID then
        return
    end
    local values = event.payload.values or {}
    if values.freq ~= nil then freq = tonumber(values.freq) or freq end
    if values.amplitude ~= nil then amplitude = tonumber(values.amplitude) or amplitude end
    if values.temp_offset ~= nil then temp_offset = tonumber(values.temp_offset) or temp_offset end
    if values.voltage_offset ~= nil then voltage_offset = tonumber(values.voltage_offset) or voltage_offset end
end)

-- ── 定时器：每 100ms 更新数据 ──

local timer_id = ctx.timer.every(100, function()
    local elapsed = (ctx.now_ms() - start_time) / 1000.0
    seq = seq + 1

    -- 正弦波
    local phase = 2 * math.pi * freq * elapsed
    local sin_val = amplitude * math.sin(phase)

    -- Chart: 多系列数据（X 轴用 elapsed 秒，便于阅读）
    ctx.bus.publish("widget.showcase.chart.sample", {
        t = elapsed,
        sine = sin_val,
        cosine = amplitude * math.cos(phase),
        saw = amplitude * (2 * (phase / (2 * math.pi) % 1) - 1)
    })

    -- Gauge: 温度（在偏移附近 ±15°C 波动）
    local temp = temp_offset + 15 * math.sin(phase * 0.3 + 1.0)
    ctx.bus.publish("widget.showcase.temperature", { value = temp })

    -- Gauge: 电压（在偏移附近 ±0.5V 波动）
    local voltage = voltage_offset + 0.5 * math.sin(phase * 0.7 + 2.0)
    ctx.bus.publish("widget.showcase.voltage", { value = voltage })

    -- Attitude: 缓慢旋转（10 秒一周）
    local roll = 20 * math.sin(phase * 0.1)
    local pitch = 15 * math.cos(phase * 0.13)
    local yaw = elapsed * 36  -- 每秒 36°
    ctx.bus.publish("widget.showcase.attitude", {
        roll = roll,
        pitch = pitch,
        yaw = yaw % 360
    })

    -- 更新状态显示
    ctx.ui.set_value(FORM_ID, "samples", tostring(seq))
end)

-- ── 清理 ──

on_disable(function()
    ctx.timer.cancel(timer_id)
    ctx.ui.remove_panel(CHART_ID)
    ctx.ui.remove_panel(GAUGE_TEMP_ID)
    ctx.ui.remove_panel(GAUGE_VOLTAGE_ID)
    ctx.ui.remove_panel(ATTITUDE_ID)
    ctx.ui.remove_panel(FORM_ID)
    ctx.log.info("Widget 展示插件停止: 共生成 " .. tostring(seq) .. " 个样本")
end)

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
    ctx.log.info("Widget 展示插件停止: 共生成 " .. tostring(seq) .. " 个样本")
end)

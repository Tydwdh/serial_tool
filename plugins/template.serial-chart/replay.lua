-- 串口图表插件模板：Replay Analyzer
--
-- 只用于回放阶段。
-- 不要访问 ctx.serial / ctx.timer / ctx.ui / ctx.bus.publish / ctx.bus.on。
--
-- 输入数据格式示例：
--   {"seq":1,"value":12.3,"target":50.0}

local TOPIC_OUT = "protocol.template.sample"

local buffers = {}
local received = 0
local parse_errors = 0
local lost_total = 0
local last_seq = nil

local function reset()
    buffers = {}
    received = 0
    parse_errors = 0
    lost_total = 0
    last_seq = nil
end

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

local function emit_sample(sample)
    received = received + 1

    if last_seq and sample.seq > last_seq + 1 then
        lost_total = lost_total + (sample.seq - last_seq - 1)
    end

    last_seq = sample.seq

    ctx.replay.emit(TOPIC_OUT, {
        t = sample.seq,
        value = sample.value,
        target = sample.target,
        received = received,
        lost_total = lost_total,
        parse_errors = parse_errors
    })
end

local function handle_line(line)
    if line == nil or line == "" then
        return
    end

    local sample = parse_packet(line)

    if sample then
        emit_sample(sample)
    else
        parse_errors = parse_errors + 1
    end
end

local function feed(port, text)
    local current = buffers[port] or ""
    current = current .. text

    while true do
        local start_index, end_index = current:find("\n", 1, true)

        if not start_index then
            break
        end

        local line = current:sub(1, start_index - 1):gsub("\r", "")
        current = current:sub(end_index + 1)

        handle_line(line)
    end

    if #current > 4096 then
        parse_errors = parse_errors + 1
        current = ""
    end

    buffers[port] = current
end

function on_replay_begin(session)
    reset()
    ctx.replay.log(
        "template.serial-chart replay started, events="
        .. tostring(session.event_count)
    )
end

function on_replay_event(event)
    if event.topic ~= "transport.serial.default.rx" then
        return
    end

    local port = "default"

    if event.metadata and event.metadata.port then
        port = tostring(event.metadata.port)
    end

    feed(port, tostring(event.payload))
end

function on_replay_end()
    ctx.replay.log(
        "template.serial-chart replay finished: received="
        .. tostring(received)
        .. ", lost="
        .. tostring(lost_total)
        .. ", parse_errors="
        .. tostring(parse_errors)
    )
end

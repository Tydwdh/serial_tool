-- Replay Analyzer: 从回放串口 RX 重建 protocol.demo.sample
--
-- 不访问 ctx.serial / ctx.timer / ctx.ui / ctx.bus.publish / ctx.bus.on
-- 只使用 ctx.replay.emit / ctx.replay.log / ctx.storage.get / ctx.plugin / ctx.now_ms
--
-- 每次 on_replay_begin 时重置内部状态

local received = 0
local parse_errors = 0

local lost_total = 0
local duplicate_total = 0
local out_of_order_total = 0
local last_rx_seq = nil

local rx_buffers = {}

local function reset_counters()
    received = 0
    parse_errors = 0
    lost_total = 0
    duplicate_total = 0
    out_of_order_total = 0
    last_rx_seq = nil
    rx_buffers = {}
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

    ctx.replay.emit("protocol.demo.sample", {
        t = sample.seq,

        target = sample.target,
        actual = sample.actual,
        output = sample.output,

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

    if #current > 4096 then
        parse_errors = parse_errors + 1
        current = ""
    end

    rx_buffers[port] = current
end

function on_replay_begin(session)
    reset_counters()
    ctx.replay.log(string.format(
        "replay analyzer started: %d events, %d..%d ms",
        session.event_count,
        session.start_ms or 0,
        session.end_ms or 0
    ))
end

function on_replay_event(event)
    if event.topic == "transport.serial.default.rx" then
        local port = ""
        if event.metadata and event.metadata.port then
            port = tostring(event.metadata.port)
        end
        -- 所有 RX 端口的数据都解析
        feed_rx_text(port, tostring(event.payload))
    end
end

function on_replay_end()
    ctx.replay.log(string.format(
        "replay analyzer finished: received=%d lost=%d dup=%d ooo=%d errors=%d",
        received, lost_total, duplicate_total, out_of_order_total, parse_errors
    ))
end

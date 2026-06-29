-- demo.gcode-sender / main.lua
-- G-code sender driven by host UI contributions.

local codec = require("hw.codec")

local PLUGIN_ID = ctx.plugin.id
local TASK_ID = "demo.gcode-sender.print"

local COMMAND = {
    SEND_FILE = "demo.gcode-sender.send_file",
    SEND_SINGLE = "demo.gcode-sender.send_single",
    PAUSE = "demo.gcode-sender.pause",
    CANCEL = "demo.gcode-sender.cancel",
}

local DEFAULTS = {
    default_setup_gcode = "M92 X40 Y40 Z2.5 E7.53",
    ack_timeout_ms = 300000,
    start_timeout_ms = 3000,
    eof_delay_ms = 1000,
    error_followup_ms = 2000,
    max_marlin_line_bytes = 96,
}

-- ── helpers ──

local function log(level, message)
    (ctx.log[level] or ctx.log.info)("[G-code] " .. tostring(message))
end

local function trim(s)
    return tostring(s or ""):gsub("^%s+", ""):gsub("%s+$", "")
end

local function sget(key)
    return ctx.config.get(key, DEFAULTS[key]) or DEFAULTS[key]
end

local function snum(key)
    return tonumber(sget(key))
end

-- 读取设置快照（一次调用，避免每行重复查 config）
local function settings()
    return {
        setup_gcode = trim(sget("default_setup_gcode")),
        ack_timeout_ms = snum("ack_timeout_ms"),
        start_timeout_ms = snum("start_timeout_ms"),
        eof_delay_ms = snum("eof_delay_ms"),
        error_followup_ms = snum("error_followup_ms"),
        max_line_bytes = snum("max_marlin_line_bytes"),
    }
end

local function send_context(payload)
    local ctx_info = (payload or {}).context or {}
    local send_info = ctx_info.send or {}
    local serial_info = ctx_info.serial or {}
    return {
        input = tostring(send_info.input or ""),
        target_port = tostring(send_info.target_port or serial_info.selected_port or ""),
        port_open = send_info.target_port_open == true,
    }
end

-- ── task helpers ──

local function current_task()
    for _, t in ipairs(ctx.task.list()) do
        if t.id == TASK_ID then
            return t
        end
    end
end

local function task_running()
    local t = current_task()
    return t and not t.finished and not t.cancelled
end

-- ── port helpers ──

local function require_open_port(port)
    if port == "" then
        log("warn", "请先在发送区选择一个已打开串口")
        return
    end

    local status = ctx.serial.status_port(port)
    if not status or not status.open then
        log("warn", "串口未打开: " .. port)
        return
    end
    return port
end

-- ── line parsing ──

---@class GCodeResponse
---@field kind "ok"|"resend"|"terminated"|"error"|"other"|"cancelled"|"timeout"
---@field line string?
---@field no integer?
---@field err string?

local function split_nonempty_lines(text)
    local result = {}
    for line in tostring(text or ""):gmatch("[^\r\n]+") do
        local clean = trim(line)
        if clean ~= "" then
            result[#result + 1] = clean
        end
    end
    return result
end

-- 一次性解析响应行
---@param line string?
---@return GCodeResponse
local function parse_response(line)
    local text = tostring(line or "")
    if text == "" then
        return { kind = "ok", line = text }
    end
    -- Resend:N / rs N
    local no = tonumber(text:match("[Rr]esend:%s*N?(%d+)"))
        or tonumber(text:match("[Rr]esend%s*N?(%d+)"))
        or tonumber(text:match("rs%s*N?(%d+)"))
    if no then
        return { kind = "resend", no = no, line = text }
    end
    -- Printer halted / Heating failed
    if text:find("Printer halted", 1, true) or text:find("Heating failed", 1, true) then
        return { kind = "terminated", line = text }
    end
    -- OK
    if text:lower():find("ok", 1, true) then
        return { kind = "ok", line = text }
    end
    -- Error
    if text:match("^Error") then
        return { kind = "error", line = text }
    end
    return { kind = "other", line = text }
end

-- ── response patterns（静态，只建一次） ──

local RESPONSE_PATTERNS = {
    { name = "ok", pattern = "ok", action = "return" },
    { name = "ok", pattern = "OK", action = "return" },
    { name = "resend", pattern = "Resend", action = "return" },
    { name = "rs", pattern = "rs ", action = "return" },
    { name = "halted", pattern = "Printer halted", action = "return" },
    { name = "heating_failed", pattern = "Heating failed", action = "return" },
    { name = "error", pattern = "^Error", action = "return" },
    { name = "busy", pattern = "busy", action = "continue" },
    { name = "dwin", pattern = "Dwin command", action = "continue" },
}

-- ── checksum ──

local function checksum_line(line, no)
    local body = "N" .. tostring(no) .. " " .. line
    return body .. "*" .. tostring(codec.xor8(body))
end

-- ── entry builders ──

local function numbered_entries(lines)
    local entries = {}
    for i, line in ipairs(lines) do
        entries[i] = { no = i, source = line, wire = checksum_line(line, i) }
    end
    return entries
end

-- ── file reading ──

local function clean_gcode_file(path, s)
    local lines = {}
    local skipped_long = 0
    local skipped_m110 = 0
    local last_clean

    -- ctx.fs.read_lines 返回迭代器函数
    local read_lines = ctx.fs.read_lines_stream or ctx.fs.read_lines
    local ok, iter = pcall(read_lines, path)
    if not ok then
        log("error", iter)
        return
    end

    for raw_line in iter do
        local clean = trim((codec.trim_line(raw_line):match("^[^;]*") or ""))
        if clean == "" or clean:sub(1, 1) == "(" then
            -- skip blank / comment
        elseif clean:find("M110", 1, true) then
            skipped_m110 = skipped_m110 + 1
        elseif #clean > s.max_line_bytes then
            skipped_long = skipped_long + 1
            log("warn", "忽略超长行 (" .. s.max_line_bytes .. "B): " .. clean)
        else
            lines[#lines + 1] = clean
            last_clean = clean
        end
    end

    -- 确保以 M2 结束
    if #lines > 0 and not (last_clean or ""):find("M2", 1, true) then
        lines[#lines + 1] = "M2"
    end

    if skipped_m110 > 0 then
        log("debug", "跳过 M110: " .. skipped_m110 .. " 行")
    end
    if skipped_long > 0 then
        log("warn", "跳过超长: " .. skipped_long .. " 行")
    end
    return lines
end

-- ── follow-up reader（error 后的补充读取） ──

---@param port string
---@param s table
---@param task HwTask
---@return GCodeResponse
local function read_followup(port, s, task)
    local deadline = ctx.now_ms() + s.error_followup_ms
    while ctx.now_ms() < deadline do
        if task:is_cancelled() then
            return { kind = "cancelled" }
        end
        local item = ctx.serial.read_line(port, { timeout_ms = 200 })
        if item and item.line then
            log("debug", "← " .. item.line)
            local r = parse_response(item.line)
            if r.kind ~= "other" then
                return r
            end
        elseif item and item.err == "cancelled" then
            return { kind = "cancelled" }
        end
    end
    return { kind = "timeout" }
end

-- ── single line send ──

---@param port string
---@param wire string
---@param s table
---@param task HwTask
---@param use_checksum boolean
---@return GCodeResponse
local function send_and_wait(port, wire, s, task, use_checksum)
    log("debug", "→ " .. wire)
    local resp = ctx.serial.write_line_and_expect(port, wire, {
        delimiter = "\n",
        timeout_ms = s.ack_timeout_ms,
        flush_before_send = false,
        patterns = RESPONSE_PATTERNS,
    })

    if resp.err then
        return { kind = "timeout", line = resp.err }
    end

    ---@type HwSerialMatchedResult
    local result = resp.result or {}
    local line = tostring(result.line or "")
    local name = tostring(result.name or "")

    if line ~= "" then
        log("debug", "← " .. line)
    end

    -- ok
    if name == "ok" then
        return { kind = "ok", line = line }
    end

    -- resend
    if name == "resend" or name == "rs" then
        return { kind = "resend", no = tonumber(line:match("N?(%d+)")) or 0, line = line }
    end

    -- terminated
    if name == "halted" or name == "heating_failed" then
        return { kind = "terminated", line = line }
    end

    -- error: 尝试恢复
    if name == "error" then
        if line:find("Unknown command", 1, true) then
            log("warn", "设备不支持该 G-code，已忽略: " .. line)
            local fu = read_followup(port, s, task)
            if fu.kind == "resend" and use_checksum then
                return fu
            end
            return { kind = "ok", line = line }
        end

        local fu = read_followup(port, s, task)
        if fu.kind == "resend" and use_checksum then
            return fu
        end
        if fu.kind == "terminated" then
            return fu
        end
        if fu.kind == "ok" and use_checksum then
            return { kind = "ok", line = line }
        end
        return { kind = "error", line = line }
    end

    return { kind = "error", line = line }
end

-- ── M110 start sync ──

local function send_start_command(port, s, task)
    local start_wire = "N0 M110 N0*" .. tostring(codec.xor8("N0 M110 N0"))
    task:set_status("同步 G-code 行号")

    for attempt = 1, 10 do
        log("debug", "→ " .. start_wire)
        local resp = ctx.serial.write_line_and_expect(port, start_wire, {
            delimiter = "\n",
            timeout_ms = s.start_timeout_ms,
            flush_before_send = false,
            patterns = RESPONSE_PATTERNS,
        })

        if resp.result and resp.result.name == "ok" then
            log("debug", "行号同步完成")
            return true
        end

        if task:is_cancelled() then
            return false
        end

        log("warn", "M110 无响应，重试 " .. attempt .. "/10")
        task:sleep_ms(3000)
    end

    log("error", "M110 同步失败，请检查串口通信")
    return false
end

-- ── main send loop ──

local function run_entries(port, entries, use_checksum, task)
    local s = settings()
    local total = #entries
    if total == 0 then
        task:set_status("没有可发送的 G-code")
        log("warn", "没有可发送的 G-code")
        return
    end

    ctx.serial.flush_rx(port)

    if use_checksum and not send_start_command(port, s, task) then
        task:set_status("行号同步失败")
        return
    end

    -- 构建行号→索引用 map（一次性）
    local index_by_no = {}
    if use_checksum then
        for i, e in ipairs(entries) do
            if e.no then
                index_by_no[e.no] = i
            end
        end
    end

    local started = ctx.now_ms()
    local pos = 1
    local max_done = 0
    task:set_progress(0, total)
    -- 初始化 contribution 进度
    ctx.ui.set_contribution_value("demo.gcode-sender.progress", { value = 0, text = "0/" .. total })

    while pos <= total do
        if task:is_cancelled() then
            task:set_status("已取消")
            ctx.ui.set_contribution_value("demo.gcode-sender.progress", { value = 0, text = "" })
            return
        end
        task:wait_if_paused()

        local entry = entries[pos]
        local label = entry.no and ("N" .. entry.no) or tostring(pos)
        task:set_status(string.format("发送 %s (%d/%d)", label, pos, total))

        local result = send_and_wait(port, entry.wire, s, task, use_checksum)

        if result.kind == "ok" then
            pos = pos + 1
            max_done = pos - 1
            task:set_progress(max_done, total)
            ctx.ui.set_contribution_value("demo.gcode-sender.progress", {
                value = max_done / total,
                text = string.format("%d/%d", max_done, total)
            })
        elseif result.kind == "resend" then
            if not use_checksum then
                log("warn", "raw 模式忽略 Resend: " .. (result.line or ""))
                pos = pos + 1
            else
                local resend_pos = result.no and index_by_no[result.no]
                if not resend_pos then
                    task:set_status("重传序号不匹配")
                    log("error", "找不到可重传序号: N" .. tostring(result.no))
                    return
                end
                log("warn", "设备请求重传 N" .. tostring(result.no))
                pos = resend_pos
            end
        elseif result.kind == "timeout" then
            log("warn", "无应答，重发当前行: " .. label)
        elseif result.kind == "terminated" then
            task:set_status("打印机错误，已停止")
            log("error", "打印机错误: " .. (result.line or ""))
            return
        elseif result.kind == "cancelled" then
            task:set_status("已取消")
            return
        else
            task:set_status("设备错误，已停止")
            log("error", "发送失败: " .. (result.line or result.kind))
            return
        end
    end

    task:set_progress(total, total)
    task:set_status("发送完成")
    ctx.ui.set_contribution_value("demo.gcode-sender.progress", {
        value = 1,
        text = string.format("%d/%d", total, total)
    })
    log("info", string.format("发送完成: %d 行", total))

    if s.eof_delay_ms > 0 then
        task:sleep_ms(s.eof_delay_ms)
    end

    local elapsed = (ctx.now_ms() - started) / 1000
    local unit = elapsed > 60 and "分钟" or "秒"
    local val = elapsed > 60 and (elapsed / 60) or elapsed
    log("info", string.format("总共耗时 %.1f %s", val, unit))
end

-- ── task lifecycle ──

local state = { paused = false, active = false }

local function start_task(port, entries, use_checksum)
    if task_running() then
        log("warn", "已有 G-code 发送任务在运行")
        return
    end

    state.paused = false
    state.active = true

    ctx.task.start({
        id = TASK_ID,
        title = "发送 G-code",
        cancellable = true,
        pausable = true,
    }, function(task)
        local ok, err = pcall(run_entries, port, entries, use_checksum, task)
        state.active = false
        state.paused = false
        if not ok then
            task:set_status("插件错误")
            log("error", err)
        end
        -- 清理 UI 属于 best-effort，不能再次覆盖 run_entries 的原始异常。
        if ctx.ui and ctx.ui.set_contribution_value then
            pcall(ctx.ui.set_contribution_value,
                "demo.gcode-sender.progress", { value = 0, text = "" })
        end
    end)
end

-- ── file path resolution ──

local function strip_quotes(s)
    local text = trim(s)
    if text:sub(1, 1) == '"' and text:sub(-1) == '"' then
        return text:sub(2, -2)
    end
    return text
end

local function looks_like_path(s)
    if s == "" or s:find("[\r\n]") then
        return false
    end
    if s:find("/", 1, true) or s:find("\\", 1, true) then
        return true
    end
    if #s >= 2 and s:sub(2, 2) == ":" then
        return true
    end
    if s:match("%.[%w_%-]+$") then
        return true
    end
    return false
end

local function resolve_file_path(input)
    local candidate = strip_quotes(input)
    if candidate == "" then
        return ctx.dialog.open_file({
            title = "选择 G-code 文件",
            filters = {
                { name = "G-code", extensions = { "gcode", "nc", "ngc", "txt" } },
                { name = "所有文件", extensions = { "*" } },
            },
        })
    end

    if looks_like_path(candidate) then
        return candidate
    end

    if candidate:find("[\r\n]") then
        log("warn", "发送区是多行内容，请使用 G单条 或 G原始 发送")
    else
        log("warn", "发送区内容不像文件路径，请清空后点 G文件 选择文件")
    end
end

-- ── command handlers ──

local function handle_send_file(payload)
    local sc = send_context(payload)
    local port = require_open_port(sc.target_port)
    if not port then return end

    local path = resolve_file_path(sc.input)
    if not path then
        log("warn", "未选择 G-code 文件")
        return
    end

    local s = settings()
    local lines = clean_gcode_file(path, s)
    if not lines or #lines == 0 then
        log("warn", "文件为空或没有有效 G-code: " .. path)
        return
    end

    log("info", string.format("开始发送: %s (%d 行)", path, #lines))
    start_task(port, numbered_entries(lines), true)
end

local function handle_send_single(payload)
    local sc = send_context(payload)
    local port = require_open_port(sc.target_port)
    if not port then return end

    local lines = split_nonempty_lines(sc.input)
    if #lines == 0 then
        log("warn", "请输入单条 G-code")
        return
    end

    local s = settings()
    local all = {}
    if s.setup_gcode ~= "" then
        all[1] = s.setup_gcode
        for i, line in ipairs(lines) do
            all[i + 1] = line
        end
    else
        for i, line in ipairs(lines) do
            all[i] = line
        end
    end
    all[#all + 1] = "M2"

    log("info", string.format("单条模式: M92=%s (%d 行)", s.setup_gcode, #all))
    start_task(port, numbered_entries(all), true)
end

local function handle_pause()
    local task = current_task()
    if not task or task.finished then
        log("warn", "没有正在运行的 G-code 任务")
        return
    end

    if state.paused then
        ctx.task.resume(TASK_ID)
        state.paused = false
        log("info", "发送已恢复")
    else
        ctx.task.pause(TASK_ID)
        state.paused = true
        log("info", "发送已暂停")
    end
end

local function handle_cancel()
    local task = current_task()
    if not task or task.finished then
        log("warn", "没有正在运行的 G-code 任务")
        return
    end

    ctx.task.cancel(TASK_ID)
    state.paused = false
    log("warn", "发送取消请求")
end

-- ── event dispatch ──

local HANDLERS = {
    [COMMAND.SEND_FILE] = handle_send_file,
    [COMMAND.SEND_SINGLE] = handle_send_single,
    [COMMAND.PAUSE] = handle_pause,
    [COMMAND.CANCEL] = handle_cancel,
}

for command, handler in pairs(HANDLERS) do
    ctx.commands.register(command, function(payload)
        handler(payload or {})
    end)
end

on_disable(function()
    if task_running() then
        ctx.task.cancel(TASK_ID)
    end
    log("info", "插件已停止")
end)

log("info", "G-code Sender ready")

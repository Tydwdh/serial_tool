-- demo.gcode-sender / main.lua
-- G-code sender driven by host UI contributions.

local codec = require("hw.codec")

local PLUGIN_ID = ctx.plugin.id
local TASK_ID = "demo.gcode-sender.print"

local COMMAND_SEND_FILE = "demo.gcode-sender.send_file"
local COMMAND_SEND_SINGLE = "demo.gcode-sender.send_single"
local COMMAND_SEND_RAW = "demo.gcode-sender.send_raw"
local COMMAND_PAUSE = "demo.gcode-sender.pause"
local COMMAND_CANCEL = "demo.gcode-sender.cancel"

local DEFAULTS = {
    default_setup_gcode = "M92 X40 Y40 Z2.5 E7.53",
    ack_timeout_ms = 300000,
    start_timeout_ms = 3000,
    send_delay_ms = 10,
    eof_delay_ms = 1000,
    error_followup_ms = 2000,
    max_marlin_line_bytes = 96,
}

local state = {
    paused = false,
    active = false,
}

local function log(level, message)
    local fn = ctx.log[level] or ctx.log.info
    fn("[G-code] " .. tostring(message))
end

local function trim(value)
    return tostring(value or ""):gsub("^%s+", ""):gsub("%s+$", "")
end

local function strip_quotes(value)
    local text = trim(value)
    if text:sub(1, 1) == '"' and text:sub(-1) == '"' then
        return text:sub(2, -2)
    end
    return text
end

local function setting_number(key)
    return tonumber(ctx.config.get(key, DEFAULTS[key])) or DEFAULTS[key]
end

local function setting_string(key)
    return tostring(ctx.config.get(key, DEFAULTS[key]) or DEFAULTS[key])
end

local function settings()
    return {
        default_setup_gcode = setting_string("default_setup_gcode"),
        ack_timeout_ms = setting_number("ack_timeout_ms"),
        start_timeout_ms = setting_number("start_timeout_ms"),
        send_delay_ms = setting_number("send_delay_ms"),
        eof_delay_ms = setting_number("eof_delay_ms"),
        error_followup_ms = setting_number("error_followup_ms"),
        max_marlin_line_bytes = setting_number("max_marlin_line_bytes"),
    }
end

local function send_context(payload)
    local context = (payload or {}).context or {}
    local send = context.send or {}
    local serial = context.serial or {}
    return {
        input = tostring(send.input or ""),
        target_port = tostring(send.target_port or serial.selected_port or ""),
        target_port_open = send.target_port_open == true,
    }
end

local function current_task()
    for _, task in ipairs(ctx.task.list()) do
        if task.id == TASK_ID then
            return task
        end
    end
    return nil
end

local function task_running()
    local task = current_task()
    return task and not task.finished and not task.cancelled
end

local function require_open_port(port)
    if port == "" then
        log("warn", "请先在发送区选择一个已打开串口")
        return nil
    end

    local status = ctx.serial.status_port(port)
    if not status or not status.open then
        log("warn", "串口未打开: " .. port)
        return nil
    end

    return port
end

local function split_nonempty_lines(text)
    local result = {}
    for line in tostring(text or ""):gmatch("[^\r\n]+") do
        local clean = trim(line)
        if clean ~= "" then
            table.insert(result, clean)
        end
    end
    return result
end

local function checksum_line(line, no)
    local body = "N" .. tostring(no) .. " " .. line
    return body .. "*" .. tostring(codec.xor8(body))
end

local function numbered_entries(lines)
    local entries = {}
    for index, line in ipairs(lines) do
        table.insert(entries, {
            no = index,
            source = line,
            wire = checksum_line(line, index),
        })
    end
    return entries
end

local function raw_entries(lines)
    local entries = {}
    for _, line in ipairs(lines) do
        table.insert(entries, {
            source = line,
            wire = line,
        })
    end
    return entries
end

local function clean_gcode_file(path, s)
    local lines = {}
    local skipped_long = 0
    local skipped_m110 = 0
    local last_clean = nil

    local read_lines = ctx.fs.read_lines_stream or ctx.fs.read_lines
    local ok, iterator = pcall(read_lines, path)
    if not ok then
        log("error", iterator)
        return nil
    end

    for raw_line in iterator do
        local line = codec.trim_line(raw_line)
        local before_comment = line:match("^[^;]*") or ""
        local clean = trim(before_comment)

        if clean == "" or clean:sub(1, 1) == "(" then
            -- skip blank/comment-only lines
        elseif clean:find("M110", 1, true) then
            skipped_m110 = skipped_m110 + 1
        elseif #clean > s.max_marlin_line_bytes then
            skipped_long = skipped_long + 1
            log("warn", "忽略超过 " .. s.max_marlin_line_bytes .. " 字节的行: " .. clean)
        else
            table.insert(lines, clean)
            last_clean = clean
        end
    end

    if #lines > 0 and not (last_clean or ""):find("M2", 1, true) then
        table.insert(lines, "M2")
    end

    if skipped_m110 > 0 then
        log("info", "已跳过 M110 行: " .. skipped_m110)
    end
    if skipped_long > 0 then
        log("warn", "已跳过超长行: " .. skipped_long)
    end

    return lines
end

local function response_patterns()
    return {
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
end

local function parse_resend_no(line)
    local text = tostring(line or "")
    return tonumber(text:match("[Rr]esend:%s*N?(%d+)"))
        or tonumber(text:match("[Rr]esend%s*N?(%d+)"))
        or tonumber(text:match("rs%s*N?(%d+)"))
end

local function classify_line(line)
    local text = tostring(line or "")
    local resend_no = parse_resend_no(text)
    if resend_no then
        return { kind = "resend", no = resend_no, line = text }
    end
    if text:find("Printer halted", 1, true) or text:find("Heating failed", 1, true) then
        return { kind = "terminated", line = text }
    end
    if text:lower():find("ok", 1, true) then
        return { kind = "ok", line = text }
    end
    if text:match("^Error") then
        return { kind = "error", line = text }
    end
    return { kind = "other", line = text }
end

local function read_followup(port, s, task)
    local deadline = ctx.now_ms() + s.error_followup_ms
    while ctx.now_ms() < deadline do
        if task:is_cancelled() then
            return { kind = "cancelled" }
        end

        local item = ctx.serial.read_line(port, { timeout_ms = 200 })
        if item and item.line then
            log("info", "接收: " .. item.line)
            local classified = classify_line(item.line)
            if classified.kind == "ok"
                or classified.kind == "resend"
                or classified.kind == "terminated" then
                return classified
            end
        elseif item and item.err == "cancelled" then
            return { kind = "cancelled" }
        end
    end
    return { kind = "timeout" }
end

local function send_and_wait(port, wire, s, task, use_checksum)
    log("info", "发送: " .. wire)
    local resp = ctx.serial.write_line_and_expect(port, wire, {
        delimiter = "\n",
        timeout_ms = s.ack_timeout_ms,
        flush_before_send = false,
        patterns = response_patterns(),
    })

    if resp.err then
        return { kind = "timeout", line = resp.err }
    end

    local result = resp.result or {}
    local line = tostring(result.line or "")
    local name = tostring(result.name or "")

    if line ~= "" then
        log("info", "接收: " .. line)
    end

    if name == "ok" then
        return { kind = "ok", line = line }
    end
    if name == "resend" or name == "rs" then
        return { kind = "resend", no = parse_resend_no(line), line = line }
    end
    if name == "halted" or name == "heating_failed" then
        return { kind = "terminated", line = line }
    end

    if name == "error" then
        if line:find("Unknown command", 1, true) then
            log("warn", "设备不支持该 G-code，已忽略: " .. line)
            local followup = read_followup(port, s, task)
            if followup.kind == "resend" and use_checksum then
                return followup
            end
            return { kind = "ok", line = line }
        end

        local followup = read_followup(port, s, task)
        if followup.kind == "resend" and use_checksum then
            return followup
        end
        if followup.kind == "terminated" then
            return followup
        end
        if followup.kind == "ok" and use_checksum then
            return { kind = "ok", line = line }
        end
        return { kind = "error", line = line }
    end

    return { kind = "error", line = line }
end

local function send_start_command(port, s, task)
    local start_body = "N0 M110 N0"
    local start_wire = start_body .. "*" .. tostring(codec.xor8(start_body))
    task:set_status("同步 G-code 行号")

    for attempt = 1, 10 do
        log("info", "发送: " .. start_wire)
        local resp = ctx.serial.write_line_and_expect(port, start_wire, {
            delimiter = "\n",
            timeout_ms = s.start_timeout_ms,
            flush_before_send = false,
            patterns = response_patterns(),
        })

        if resp.result and resp.result.name == "ok" then
            log("info", "行号同步完成")
            return true
        end

        if task:is_cancelled() then
            return false
        end

        log("warn", "M110 无响应，重试 " .. attempt .. "/10")
        task:sleep_ms(3000)
    end

    log("error", "请检查和打印机的串口通信，终止")
    return false
end

local function run_entries(port, entries, use_checksum, task)
    local s = settings()
    local started_ms = ctx.now_ms()
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

    local index_by_no = {}
    for index, entry in ipairs(entries) do
        if entry.no then
            index_by_no[entry.no] = index
        end
    end

    local pos = 1
    local max_done = 0
    task:set_progress(0, total)

    while pos <= total do
        if task:is_cancelled() then
            task:set_status("已取消")
            log("warn", "发送已取消")
            return
        end

        task:wait_if_paused()

        local entry = entries[pos]
        local label = entry.no and ("N" .. tostring(entry.no)) or tostring(pos)
        task:set_status("发送 " .. label .. " (" .. pos .. "/" .. total .. ")")

        local result = send_and_wait(port, entry.wire, s, task, use_checksum)

        if result.kind == "ok" then
            pos = pos + 1
            if pos - 1 > max_done then
                max_done = pos - 1
            end
            task:set_progress(max_done, total)
            if s.send_delay_ms > 0 then
                task:sleep_ms(s.send_delay_ms)
            end
        elseif result.kind == "resend" then
            if not use_checksum then
                log("warn", "raw 模式忽略 Resend: " .. tostring(result.line or ""))
                pos = pos + 1
            else
                local no = result.no or entry.no
                local resend_pos = no and index_by_no[no] or nil
                if not resend_pos then
                    task:set_status("重传序号不匹配")
                    log("error", "找不到可重传的 G-code 序号: " .. tostring(no))
                    return
                end
                log("warn", "设备请求重传 N" .. tostring(no))
                pos = resend_pos
            end
        elseif result.kind == "timeout" then
            log("warn", "无应答，继续重发当前行: " .. label)
        elseif result.kind == "terminated" then
            task:set_status("打印机错误，已停止")
            log("error", "打印机错误，已停止: " .. tostring(result.line or ""))
            return
        elseif result.kind == "cancelled" then
            task:set_status("已取消")
            return
        else
            task:set_status("设备错误，已停止")
            log("error", "设备错误，已停止: " .. tostring(result.line or result.kind))
            return
        end
    end

    task:set_progress(total, total)
    task:set_status("发送完成")
    log("info", "发送完成: " .. total .. " 行")

    if s.eof_delay_ms > 0 then
        task:sleep_ms(s.eof_delay_ms)
    end

    local elapsed = (ctx.now_ms() - started_ms) / 1000
    if elapsed > 60 then
        log("warn", string.format("总共耗时 %.2f 分钟", elapsed / 60))
    else
        log("warn", string.format("总共耗时 %.2f 秒", elapsed))
    end
end

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
    end)
end

local function open_gcode_file_dialog()
    return ctx.dialog.open_file({
        title = "选择 G-code 文件",
        filters = {
            { name = "G-code", extensions = { "gcode", "nc", "ngc", "txt" } },
            { name = "所有文件", extensions = { "*" } },
        },
    })
end

local function looks_like_file_path(value)
    local text = strip_quotes(value)
    if text == "" or text:find("[\r\n]") then
        return false
    end

    if text:find("/", 1, true) or text:find("\\", 1, true) then
        return true
    end
    if #text >= 2 and text:sub(2, 2) == ":" then
        return true
    end
    if text:match("%.[%w_%-]+$") then
        return true
    end

    return false
end

local function resolve_file_path(input)
    local candidate = strip_quotes(input)
    if candidate == "" then
        return open_gcode_file_dialog()
    end

    if looks_like_file_path(candidate) then
        return candidate
    end

    if candidate:find("[\r\n]") then
        log("warn", "发送区是多行内容，不是文件路径。发送当前内容请使用 G单条 或 G原始。")
    else
        log(
            "warn",
            "发送区内容不像文件路径: "
                .. candidate
                .. "。发送当前 G-code 请使用 G单条 或 G原始；发送文件请清空发送区后点 G文件。"
        )
    end

    return nil
end

local function handle_send_file(payload)
    local sc = send_context(payload)
    local port = require_open_port(sc.target_port)
    if not port then
        return
    end

    local path = resolve_file_path(sc.input)
    if not path then
        log("warn", "未选择 G-code 文件")
        return
    end

    local s = settings()
    local lines = clean_gcode_file(path, s)
    if not lines or #lines == 0 then
        log("warn", "文件为空或没有有效 G-code: " .. tostring(path))
        return
    end

    log("info", "开始打印文件: " .. path .. " (" .. #lines .. " 行)")
    start_task(port, numbered_entries(lines), true)
end

local function handle_send_single(payload)
    local sc = send_context(payload)
    local port = require_open_port(sc.target_port)
    if not port then
        return
    end

    local command_lines = split_nonempty_lines(sc.input)
    if #command_lines == 0 then
        log("warn", "请输入单条 G-code")
        return
    end

    local s = settings()
    local lines = {}
    local setup = trim(s.default_setup_gcode)
    if setup ~= "" then
        table.insert(lines, setup)
    end
    for _, line in ipairs(command_lines) do
        table.insert(lines, line)
    end
    table.insert(lines, "M2")

    log("warn", "单条模式会先发送默认 M92，请确认配置: " .. setup)
    start_task(port, numbered_entries(lines), true)
end

local function handle_send_raw(payload)
    local sc = send_context(payload)
    local port = require_open_port(sc.target_port)
    if not port then
        return
    end

    local lines = split_nonempty_lines(sc.input)
    if #lines == 0 then
        log("warn", "请输入原始 G-code")
        return
    end

    start_task(port, raw_entries(lines), false)
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

ctx.bus.on("ui.contribution.action", function(event)
    local payload = event.payload or {}
    if payload.plugin_id ~= PLUGIN_ID then
        return
    end

    local action = payload.action or payload.command
    if action == COMMAND_SEND_FILE then
        handle_send_file(payload)
    elseif action == COMMAND_SEND_SINGLE then
        handle_send_single(payload)
    elseif action == COMMAND_SEND_RAW then
        handle_send_raw(payload)
    elseif action == COMMAND_PAUSE then
        handle_pause()
    elseif action == COMMAND_CANCEL then
        handle_cancel()
    end
end)

on_disable(function()
    if task_running() then
        ctx.task.cancel(TASK_ID)
    end
    log("info", "插件已停止")
end)

log("info", "G-code Sender ready. Actions are contributed to send.toolbar")

-- gcode-sender / main.lua
-- G-code sender driven by host UI contributions.

local codec = require("hw.codec")

local PLUGIN_ID = ctx.plugin.id
local TASK_ID = "gcode-sender.print"

local COMMAND = {
    SEND_FILE = "gcode-sender.send_file",
    SEND_SINGLE = "gcode-sender.send_single",
    PAUSE = "gcode-sender.pause",
    CANCEL = "gcode-sender.cancel",
}

local DEFAULTS = {
    default_setup_gcode = "M92 X40 Y40 Z2.5 E7.53",
    ack_timeout_ms = 300000,
    start_timeout_ms = 3000,
    eof_delay_ms = 1000,
    error_followup_ms = 2000,
    max_marlin_line_bytes = 96,
    max_retries = 3,
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

local function bounded_int(key, minimum, maximum)
    local value = tonumber(sget(key)) or DEFAULTS[key]
    value = math.floor(value + 0.5)
    return math.max(minimum, math.min(maximum, value))
end

-- 读取设置快照（一次调用，避免每行重复查 config）
local function settings()
    return {
        setup_gcode = trim(sget("default_setup_gcode")),
        ack_timeout_ms = bounded_int("ack_timeout_ms", 1000, 600000),
        start_timeout_ms = bounded_int("start_timeout_ms", 500, 30000),
        eof_delay_ms = bounded_int("eof_delay_ms", 0, 10000),
        error_followup_ms = bounded_int("error_followup_ms", 100, 10000),
        max_line_bytes = bounded_int("max_marlin_line_bytes", 32, 256),
        max_retries = bounded_int("max_retries", 0, 20),
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

local function ensure_idle()
    if not task_running() then
        return true
    end
    log("warn", "已有 G-code 发送任务在运行")
    ctx.ui.set_status("[G-code] 已有发送任务在运行，请等待完成或取消")
    return false
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
    -- Resend:N / rs N（Marlin 实际格式 "Resend: N123"，也兼容 "Resend:N123"/"Resend: 123"）
    local no = tonumber(text:match("[Rr]esend:%s*N?(%d+)"))
        or tonumber(text:match("[Rr]esend%s*N?(%d+)"))
        or tonumber(text:match("^rs%s+N?(%d+)"))
    if no then
        return { kind = "resend", no = no, line = text }
    end
    -- Printer halted / Heating failed
    if text:find("Printer halted", 1, true) or text:find("Heating failed", 1, true) then
        return { kind = "terminated", line = text }
    end
    -- OK（必须行首锚定，避免 "rookie"/"Bed OK"/"echo: ok" 等含 ok 子串的行误判）
    if text:lower():match("^ok") then
        return { kind = "ok", line = text }
    end
    -- Error
    if text:match("^Error") then
        return { kind = "error", line = text }
    end
    return { kind = "other", line = text }
end

-- ── response patterns（静态，只建一次） ──
-- pattern 经宿主 match_pat 处理：^ 前缀 = starts_with，否则 = contains。
-- ok/resend/error 必须用 ^ 锚定，避免含子串的设备回显误匹配。

local RESPONSE_PATTERNS = {
    { name = "ok", pattern = "^ok", action = "return" },
    { name = "ok", pattern = "^OK", action = "return" }, -- 兼容 Repetier 等大写 ack
    { name = "ok", pattern = "^Ok", action = "return" },
    { name = "ok", pattern = "^oK", action = "return" },
    { name = "resend", pattern = "^Resend:", action = "return" },
    { name = "rs", pattern = "^rs ", action = "return" },
    { name = "halted", pattern = "^Printer halted", action = "return" },
    { name = "heating_failed", pattern = "^Heating failed", action = "return" },
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

local function numbered_entries(lines, max_wire_bytes)
    local entries = {}
    local skipped = 0
    for _, line in ipairs(lines) do
        local no = #entries + 1
        local wire = checksum_line(line, no)
        if #wire > max_wire_bytes then
            skipped = skipped + 1
            if skipped <= 3 then
                log("warn", string.format("忽略超长发送行 (%dB > %dB): %s",
                    #wire, max_wire_bytes, line))
            end
        else
            entries[#entries + 1] = { no = no, source = line, wire = wire }
        end
    end
    if skipped > 0 then
        log("warn", "共跳过超长发送行: " .. skipped .. " 行")
    end
    return entries
end

-- ── file reading ──

-- 移除 G-code 括号注释 (...)。G-code 标准不支持嵌套括号，%b() 平衡匹配即可。
-- 必须在算校验和前移除：Marlin 解析时移除括号内容，若原文带括号会导致校验和不一致。
local function strip_parens(s)
    return (s:gsub("%b()", ""))
end

-- 清理注释，并移除文件中已有的 Marlin 行号和校验和；插件会统一重新编号。
local function normalize_gcode_line(raw_line)
    local clean = trim(strip_parens(raw_line):match("^[^;]*") or "")
    clean = trim((clean:gsub("%*%d+%s*$", "")))
    clean = trim((clean:gsub("^[Nn]%d+%s+", "", 1)))
    return clean
end

local function command_word(line)
    local word = tostring(line or ""):match("^([GgMmTt]%d+)")
    return word and word:upper() or ""
end

local function is_program_end(line)
    local command = command_word(line)
    return command == "M2" or command == "M30"
end

local function append_clean_line(lines, raw_line, s, stats)
    local clean = normalize_gcode_line(raw_line)
    if clean == "" then
        return
    end
    if command_word(clean) == "M110" then
        stats.skipped_m110 = stats.skipped_m110 + 1
        return
    end
    if #clean > s.max_line_bytes then
        stats.skipped_long = stats.skipped_long + 1
        if stats.skipped_long <= 3 then
            log("warn", "忽略超长行 (" .. s.max_line_bytes .. "B): " .. clean)
        end
        return
    end
    lines[#lines + 1] = clean
end

local function finish_clean_lines(lines, stats)
    if #lines > 0 and not is_program_end(lines[#lines]) then
        lines[#lines + 1] = "M2"
    end
    if stats.skipped_m110 > 0 then
        log("debug", "跳过文件内 M110: " .. stats.skipped_m110 .. " 行")
    end
    if stats.skipped_long > 0 then
        log("warn", "共跳过超长 G-code: " .. stats.skipped_long .. " 行")
    end
    return lines
end

local function clean_gcode_file(path, s)
    local lines = {}
    local stats = { skipped_long = 0, skipped_m110 = 0 }

    -- ctx.fs.read_lines 返回迭代器函数
    local read_lines = ctx.fs.read_lines_stream or ctx.fs.read_lines
    local ok, iter = pcall(read_lines, path)
    if not ok then
        log("error", iter)
        return
    end

    for raw_line in iter do
        append_clean_line(lines, raw_line, s, stats)
    end
    return finish_clean_lines(lines, stats)
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

    if task:is_cancelled() or resp.err == "cancelled" then
        return { kind = "cancelled", line = resp.err }
    end
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

    -- resend（序号提取与 parse_response 一致：锚定到 : 或空格后的数字）
    if name == "resend" or name == "rs" then
        local no = tonumber(line:match(":%s*N?(%d+)"))
            or tonumber(line:match("%sN?(%d+)"))
            or 0
        return { kind = "resend", no = no, line = line }
    end

    -- terminated
    if name == "halted" or name == "heating_failed" then
        return { kind = "terminated", line = line }
    end

    -- error: 尝试恢复
    if name == "error" then
        -- Unknown command 往往是前一行错位/截断导致，不应当 ok 跳过（会让错位累积）。
        -- 当作 timeout 处理：主循环会重发当前行，给设备一次纠正机会。
        if line:find("Unknown command", 1, true) then
            log("warn", "设备报告未知命令，重发当前行: " .. line)
            return { kind = "timeout", line = line }
        end

        local fu = read_followup(port, s, task)
        if fu.kind == "resend" and use_checksum then
            return fu
        end
        if fu.kind == "terminated" then
            return fu
        end
        -- followup 收到 ok 不能吞掉 error：可能是上一行迟到的 stale ok。
        -- 保守按 error 处理，避免掩盖真实的设备错误（如加热失败）。
        return { kind = "error", line = line }
    end

    return { kind = "error", line = line }
end

-- ── M110 start sync ──

local function send_start_command(port, s, task)
    local start_wire = "N0 M110 N0*" .. tostring(codec.xor8("N0 M110 N0"))
    task:set_status("同步 G-code 行号")

    -- 每次重试前 flush，清掉上次可能的 stale 响应。
    local max_attempts = s.max_retries + 1
    for attempt = 1, max_attempts do
        if attempt > 1 then
            ctx.serial.flush_rx(port)
        end
        log("debug", "→ " .. start_wire)
        local resp = ctx.serial.write_line_and_expect(port, start_wire, {
            delimiter = "\n",
            timeout_ms = s.start_timeout_ms,
            flush_before_send = false, -- 循环内已显式 flush
            patterns = RESPONSE_PATTERNS,
        })

        if resp.result and resp.result.name == "ok" then
            log("debug", "行号同步完成")
            return true
        end

        if task:is_cancelled() then
            return false
        end

        if attempt < max_attempts then
            log("warn", string.format("M110 无响应，将重试 %d/%d",
                attempt, s.max_retries))
            -- write_line_and_expect 已经等待过 start_timeout_ms，只需短暂让出任务。
            task:sleep_ms(100)
        end
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
    local retry_count = 0
    task:set_progress(0, total)
    -- 初始化 contribution 进度
    ctx.ui.set_contribution_value("gcode-sender.progress", { value = 0, text = "0/" .. total })

    while pos <= total do
        if task:is_cancelled() then
            task:set_status("已取消")
            ctx.ui.set_contribution_value("gcode-sender.progress", { value = 0, text = "" })
            return
        end
        task:wait_if_paused()
        -- 取消会解除暂停；恢复执行后必须再次检查，避免额外下发一行。
        if task:is_cancelled() then
            task:set_status("已取消")
            return
        end

        local entry = entries[pos]
        local label = entry.no and ("N" .. entry.no) or tostring(pos)
        task:set_status(string.format("发送 %s (%d/%d)", label, pos, total))

        local result = send_and_wait(port, entry.wire, s, task, use_checksum)

        if result.kind == "ok" then
            retry_count = 0
            pos = pos + 1
            max_done = pos - 1
            task:set_progress(max_done, total)
            ctx.ui.set_contribution_value("gcode-sender.progress", {
                value = max_done / total,
                text = string.format("%d/%d", max_done, total)
            })
        elseif result.kind == "resend" then
            if not use_checksum then
                log("warn", "raw 模式忽略 Resend: " .. (result.line or ""))
                retry_count = 0
                pos = pos + 1
            else
                retry_count = retry_count + 1
                if retry_count > s.max_retries then
                    task:set_status("重传次数过多，已停止")
                    log("error", string.format("设备连续请求重传，超过上限 %d 次: %s",
                        s.max_retries, result.line or label))
                    return
                end
                log("warn", string.format("设备请求重传（%d/%d）: %s",
                    retry_count, s.max_retries, result.line or label))
                ctx.serial.flush_rx(port)

                if result.no == 0 then
                    -- Resend:0 表示设备请求重传 M110（N0），不在 entries 里。
                    -- 重新执行 M110 行号同步，然后从第 1 行继续。
                    if send_start_command(port, s, task) then
                        pos = 1
                    else
                        task:set_status("行号重新同步失败")
                        return
                    end
                else
                    local resend_pos = result.no and index_by_no[result.no]
                    if not resend_pos then
                        task:set_status("重传序号不匹配")
                        log("error", "找不到可重传序号: N" .. tostring(result.no))
                        return
                    end
                    pos = resend_pos
                end
            end
        elseif result.kind == "timeout" then
            retry_count = retry_count + 1
            if retry_count > s.max_retries then
                task:set_status("ACK 超时，已停止")
                log("error", string.format("%s 连续无应答，超过重试上限 %d 次",
                    label, s.max_retries))
                return
            end
            log("warn", string.format("%s 无应答，重试 %d/%d",
                label, retry_count, s.max_retries))
            ctx.serial.flush_rx(port)
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
    ctx.ui.set_contribution_value("gcode-sender.progress", {
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

local function start_task(port, entries, use_checksum)
    if not ensure_idle() then
        return
    end

    ctx.task.start({
        id = TASK_ID,
        title = "发送 G-code",
        cancellable = true,
        pausable = true,
    }, function(task)
        local ok, err = pcall(run_entries, port, entries, use_checksum, task)
        if not ok then
            task:set_status("插件错误")
            log("error", err)
        end
        -- 清理 UI 属于 best-effort，不能再次覆盖 run_entries 的原始异常。
        if ctx.ui and ctx.ui.set_contribution_value then
            pcall(ctx.ui.set_contribution_value,
                "gcode-sender.progress", { value = 0, text = "" })
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
    if not ensure_idle() then return end
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
    start_task(port, numbered_entries(lines, s.max_line_bytes), true)
end

local function handle_send_single(payload)
    if not ensure_idle() then return end
    local sc = send_context(payload)
    local port = require_open_port(sc.target_port)
    if not port then return end

    local input_lines = split_nonempty_lines(sc.input)
    if #input_lines == 0 then
        log("warn", "请输入单条 G-code")
        return
    end

    local s = settings()
    local all = {}
    local stats = { skipped_long = 0, skipped_m110 = 0 }
    if s.setup_gcode ~= "" then
        for _, line in ipairs(split_nonempty_lines(s.setup_gcode)) do
            append_clean_line(all, line, s, stats)
        end
    end
    for _, line in ipairs(input_lines) do
        append_clean_line(all, line, s, stats)
    end
    finish_clean_lines(all, stats)

    if #all == 0 then
        log("warn", "输入中没有可发送的 G-code")
        return
    end

    log("info", string.format("单条模式: 初始化=%s (%d 行)", s.setup_gcode, #all))
    start_task(port, numbered_entries(all, s.max_line_bytes), true)
end

local function handle_pause()
    local task = current_task()
    if not task or task.finished then
        log("warn", "没有正在运行的 G-code 任务")
        return
    end

    if task.paused then
        ctx.task.resume(TASK_ID)
        log("info", "发送已恢复")
    else
        ctx.task.pause(TASK_ID)
        log("info", "发送已暂停")
    end
end

local function handle_cancel()
    local task = current_task()
    if not task or task.finished or task.cancelled then
        log("warn", "没有正在运行的 G-code 任务")
        return
    end

    ctx.task.cancel(TASK_ID)
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

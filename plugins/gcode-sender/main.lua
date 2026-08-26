-- gcode-sender / main.lua
-- G-code sender driven by host UI contributions.

local codec = require("hw.codec")

local PLUGIN_ID = ctx.plugin.id
local TASK_ID = "gcode-sender.print"
local LEGACY_SETUP_GCODE = "M92 X40 Y40 Z2.5 E7.53"
local active_port

local COMMAND = {
    SEND_FILE = "gcode-sender.send_file",
    SEND_SINGLE = "gcode-sender.send_single",
    PAUSE = "gcode-sender.pause",
    CANCEL = "gcode-sender.cancel",
}

local DEFAULTS = {
    default_setup_gcode = "",
    append_program_end = true,
    omit_line_numbers = false,
    skip_line_number_sync = false,
    ack_timeout_ms = 300000,
    start_timeout_ms = 3000,
    error_followup_ms = 2000,
    custom_success_patterns = "",
    custom_error_patterns = "",
    custom_running_patterns = "",
    pause_gcode = "",
    cancel_gcode = "",
}

-- ── helpers ──

local function log(level, message)
    (ctx.log[level] or ctx.log.info)("[G-code] " .. tostring(message))
end

local function trim(s)
    return tostring(s or ""):gsub("^%s+", ""):gsub("%s+$", "")
end

local function sget(key)
    local value = ctx.config.get(key, DEFAULTS[key])
    if value == nil then
        return DEFAULTS[key]
    end
    return value
end

local function integer_setting(key)
    local value = tonumber(sget(key)) or DEFAULTS[key]
    return math.floor(value + 0.5)
end

-- 将 textarea 多行内容拆成正则列表；空行与 # 注释行忽略。
local function parse_pattern_lines(text)
    local result = {}
    for line in tostring(text or ""):gmatch("[^\r\n]+") do
        local pat = trim(line)
        if pat ~= "" and not pat:match("^#") then
            result[#result + 1] = pat
        end
    end
    return result
end

-- 读取设置快照（一次调用，避免每行重复查 config）
local function settings()
    return {
        setup_gcode = trim(sget("default_setup_gcode")),
        append_program_end = sget("append_program_end") == true,
        omit_line_numbers = sget("omit_line_numbers") == true,
        skip_line_number_sync = sget("skip_line_number_sync") == true,
        ack_timeout_ms = integer_setting("ack_timeout_ms"),
        start_timeout_ms = integer_setting("start_timeout_ms"),
        error_followup_ms = integer_setting("error_followup_ms"),
        success_patterns = parse_pattern_lines(sget("custom_success_patterns")),
        error_patterns = parse_pattern_lines(sget("custom_error_patterns")),
        running_patterns = parse_pattern_lines(sget("custom_running_patterns")),
        pause_gcode = tostring(sget("pause_gcode")),
        cancel_gcode = tostring(sget("cancel_gcode")),
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
    local text = trim(tostring(line or ""):gsub("^%([^%)]*%)", "", 1))
    local lower = text:lower()
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
    if lower:find("printer halted", 1, true)
        or lower:find("heating failed", 1, true)
        or lower:match("^!!")
        or lower:match("^alarm")
        or lower:match("^stopped") then
        return { kind = "terminated", line = text }
    end
    -- OK（必须行首锚定，避免 "rookie"/"Bed OK"/"echo: ok" 等含 ok 子串的行误判）
    if lower:match("^ok") then
        return { kind = "ok", line = text }
    end
    -- Error
    if lower:match("^error") then
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
    { name = "error", pattern = "^error", action = "return" },
    { name = "error", pattern = "^ERROR", action = "return" },
    { name = "halted", pattern = "^!!", action = "return" },
    { name = "halted", pattern = "^ALARM", action = "return" },
    { name = "halted", pattern = "^Stopped", action = "return" },
    { name = "busy", pattern = "busy", action = "continue" },
    { name = "dwin", pattern = "Dwin command", action = "continue" },
}

-- ── checksum ──

local function checksum_line(line, no)
    local body = "N" .. tostring(no) .. " " .. line
    return body .. "*" .. tostring(codec.xor8(body))
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

local function new_entry_builder(s)
    return {
        settings = s,
        entries = {},
        skipped_m110 = 0,
        last_clean = "",
        content_count = 0,
    }
end

local function migrate_legacy_config()
    local value = trim(ctx.config.get("default_setup_gcode", ""))
    if value ~= LEGACY_SETUP_GCODE then
        return
    end
    local ok, err = pcall(ctx.config.set, "default_setup_gcode", "")
    if ok then
        log("warn", "已清除旧版内置 M92 初始化值；如设备确实需要，请在插件设置中重新填写")
    else
        log("error", "无法迁移旧版初始化设置: " .. tostring(err))
    end
end

local function origin_label(origin, source_line)
    if source_line then
        return string.format("%s第 %d 行", origin, source_line)
    end
    return origin
end

local function append_clean_entry(builder, raw_line, origin, source_line, is_content)
    local clean = normalize_gcode_line(raw_line)
    if clean == "" then
        return
    end
    if command_word(clean) == "M110" then
        builder.skipped_m110 = builder.skipped_m110 + 1
        return
    end

    if is_content then
        builder.content_count = builder.content_count + 1
    end
    builder.last_clean = clean
    local no = #builder.entries + 1
    -- omit_line_numbers：发送原始行（依赖固件无校验模式）；否则加行号与校验和。
    local wire
    if builder.settings.omit_line_numbers then
        wire = clean
    else
        wire = checksum_line(clean, no)
    end

    builder.entries[#builder.entries + 1] = {
        no = builder.settings.omit_line_numbers and nil or no,
        source = clean,
        wire = wire,
        origin = origin,
        source_line = source_line,
    }
end

local function append_setup_entries(builder, setup_gcode)
    if setup_gcode == "" then
        return
    end
    for index, line in ipairs(split_nonempty_lines(setup_gcode)) do
        append_clean_entry(builder, line, "初始化命令 " .. index, nil, false)
    end
end

local function finish_entries(builder)
    if builder.settings.append_program_end
        and builder.content_count > 0
        and builder.last_clean ~= ""
        and not is_program_end(builder.last_clean) then
        append_clean_entry(builder, "M2", "自动结束命令", nil, false)
    end
    if builder.skipped_m110 > 0 then
        log("debug", "跳过输入中的 M110: " .. builder.skipped_m110 .. " 行")
    end
    return builder
end

local function entries_from_text(text, s)
    local builder = new_entry_builder(s)
    append_setup_entries(builder, s.setup_gcode)
    for index, line in ipairs(split_nonempty_lines(text)) do
        append_clean_entry(builder, line, "输入第 " .. index .. " 行", nil, true)
    end
    return finish_entries(builder)
end

local function entries_from_file(path, s, task)
    local builder = new_entry_builder(s)
    append_setup_entries(builder, s.setup_gcode)

    local read_lines = ctx.fs.read_lines_stream or ctx.fs.read_lines
    local source_line = 0
    local ok, err = pcall(function()
        local iter = read_lines(path)
        for raw_line in iter do
            source_line = source_line + 1
            append_clean_entry(builder, raw_line, "文件", source_line, true)
            if source_line % 1000 == 0 then
                task:set_status(string.format("正在检查文件（%d 行）", source_line))
            end
        end
    end)
    if not ok then
        task:set_status("读取 G-code 文件失败")
        log("error", string.format("读取失败（文件第 %d 行附近）: %s", source_line + 1, err))
        return
    end

    return finish_entries(builder)
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

---@param s table 设置快照（含 success_patterns / error_patterns）
---@return table patterns 表：内置 RESPONSE_PATTERNS + 用户自定义正则（re: 前缀）
local function response_patterns(s)
    local patterns = {}
    for _, item in ipairs(RESPONSE_PATTERNS) do
        patterns[#patterns + 1] = item
    end
    -- 用户自定义成功/错误正则：命中即视为 ok / error，走既有分支逻辑。
    for _, re in ipairs(s.success_patterns) do
        patterns[#patterns + 1] = {
            name = "ok",
            pattern = "re:" .. re,
            action = "return",
        }
    end
    for _, re in ipairs(s.error_patterns) do
        patterns[#patterns + 1] = {
            name = "error",
            pattern = "re:" .. re,
            action = "return",
        }
    end
    -- 设备仍在执行当前命令时，用 continue 刷新等待窗口，不把它当作 ACK。
    for _, re in ipairs(s.running_patterns) do
        patterns[#patterns + 1] = {
            name = "running",
            pattern = "re:" .. re,
            action = "continue",
        }
    end
    return patterns
end

---@param port string
---@param wire string
---@param s table
---@param task HwTask
---@param timeout_ms integer?
---@return GCodeResponse
local function send_and_wait(port, wire, s, task, timeout_ms)
    log("debug", "→ " .. wire)
    local resp = ctx.serial.write_line_and_expect(port, wire, {
        delimiter = "\n",
        timeout_ms = timeout_ms or s.ack_timeout_ms,
        continue_resets_timeout = true,
        flush_before_send = false,
        patterns = response_patterns(s),
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
    local parsed = parse_response(line)

    if line ~= "" then
        log("debug", "← " .. line)
    end

    -- ok
    if name == "ok" then
        return { kind = "ok", line = line }
    end

    -- resend（序号提取与 parse_response 一致：锚定到 : 或空格后的数字）
    if name == "resend" or name == "rs" then
        return {
            kind = "resend",
            no = parsed.no or 0,
            line = parsed.line or line,
        }
    end

    -- terminated
    if name == "halted" or name == "heating_failed" then
        return { kind = "terminated", line = parsed.line or line }
    end

    -- error: 尝试恢复
    if name == "error" then
        if parsed.kind == "terminated" then
            return parsed
        end
        if line:lower():find("unknown command", 1, true) then
            return { kind = "error", line = parsed.line or line }
        end

        local fu = read_followup(port, s, task)
        if fu.kind == "resend" then
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

    local result = send_and_wait(port, start_wire, s, task, s.start_timeout_ms)
    if result.kind == "ok" then
        log("debug", "行号同步完成")
        return true
    end

    if result.kind == "terminated" or result.kind == "error" then
        log("error", "M110 同步被设备拒绝: " .. (result.line or result.kind))
    elseif result.kind == "timeout" then
        log("error", "M110 同步超时，请检查串口通信")
    else
        log("error", "M110 同步失败: " .. (result.line or result.kind))
    end
    return false
end

-- ── main send loop ──

local function run_entries(port, entries, s, task)
    local total = #entries
    if total == 0 then
        task:set_status("没有可发送的 G-code")
        log("warn", "没有可发送的 G-code")
        return
    end

    ctx.serial.flush_rx(port)

    -- 无校验模式或显式跳过同步时，不发送 N0 M110 N0；后续仍可保留行号校验。
    if not s.omit_line_numbers and not s.skip_line_number_sync then
        if not send_start_command(port, s, task) then
            task:set_status("行号同步失败")
            return
        end
    end

    -- 构建行号→索引用 map（一次性）；无校验模式没有行号，无需构建
    local index_by_no = {}
    if not s.omit_line_numbers then
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
    ctx.ui.set_contribution_value("gcode-sender.progress", {
        value = 0,
        text = "已确认 0/" .. total,
    })

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
        local location = origin_label(entry.origin, entry.source_line)
        task:set_status(string.format("发送 %s · %s (%d/%d)", label, location, pos, total))

        local result = send_and_wait(port, entry.wire, s, task)

        if result.kind == "ok" then
            pos = pos + 1
            max_done = pos - 1
            task:set_progress(max_done, total)
            ctx.ui.set_contribution_value("gcode-sender.progress", {
                value = max_done / total,
                text = string.format("已确认 %d/%d", max_done, total)
            })
        elseif result.kind == "resend" then
            if s.omit_line_numbers then
                -- 无校验模式没有行号体系，无法定位重传位置，只能停止。
                task:set_status("设备请求重传但未启用行号校验")
                log("error", string.format("无校验模式下收到重传请求（%s）: %s",
                    label, result.line or ""))
                return
            end
            if result.no == total + 1 then
                -- ACK 丢失后重发最后一行时，Marlin 可能告知下一期待序号。
                pos = total + 1
            else
                log("warn", "设备请求重传: " .. (result.line or label))
                ctx.serial.flush_rx(port)

                if result.no == 0 then
                    -- Resend:0 表示设备请求重传 M110（N0），不在 entries 里。
                    -- 重新执行 M110 行号同步，然后从第 1 行继续。
                    if s.skip_line_number_sync then
                        task:set_status("设备请求 M110，但已跳过行号同步")
                        log("error", "设备请求重传 M110，但当前配置禁止发送 M110")
                        return
                    end
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
            task:set_status("ACK 超时，已停止")
            log("error", string.format("%s（%s）无应答，发送已停止",
                label, location))
            return
        elseif result.kind == "terminated" then
            task:set_status("打印机错误，已停止")
            log("error", string.format("打印机错误（%s）: %s", location, result.line or ""))
            return
        elseif result.kind == "cancelled" then
            task:set_status("已取消")
            return
        else
            task:set_status("设备错误，已停止")
            log("error", string.format("发送失败（%s）: %s",
                location, result.line or result.kind))
            return
        end
    end

    task:set_progress(total, total)
    task:set_status("发送完成")
    ctx.ui.set_contribution_value("gcode-sender.progress", {
        value = 1,
        text = string.format("已确认 %d/%d", total, total)
    })
    log("info", string.format("发送完成: %d 行", total))

    local elapsed = (ctx.now_ms() - started) / 1000
    local unit = elapsed > 60 and "分钟" or "秒"
    local val = elapsed > 60 and (elapsed / 60) or elapsed
    log("info", string.format("总共耗时 %.1f %s", val, unit))
end

-- ── task lifecycle ──

local function start_task(port, prepare)
    if not ensure_idle() then
        return
    end

    active_port = port
    ctx.task.start({
        id = TASK_ID,
        title = "发送 G-code",
        cancellable = true,
        pausable = true,
    }, function(task)
        local ok, err = pcall(function()
            local s = settings()
            local builder = prepare(s, task)
            if not builder then
                return
            end
            if builder.content_count == 0 then
                task:set_status("没有可发送的 G-code")
                log("warn", "输入中没有可发送的 G-code")
                return
            end
            run_entries(port, builder.entries, s, task)
        end)
        if not ok then
            task:set_status("插件错误")
            log("error", err)
        end
        -- 清理 UI 属于 best-effort，不能再次覆盖 run_entries 的原始异常。
        if ctx.ui and ctx.ui.set_contribution_value then
            pcall(ctx.ui.set_contribution_value,
                "gcode-sender.progress", { value = 0, text = "" })
        end
        active_port = nil
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
                { name = "Marlin G-code", extensions = { "gcode", "gco", "gc" } },
                { name = "所有文件", extensions = { "*" } },
            },
        })
    end

    if looks_like_path(candidate) then
        return candidate
    end

    if candidate:find("[\r\n]") then
        log("warn", "发送区是多行内容，请使用 G单条 发送")
    else
        log("warn", "发送区内容不像文件路径，请清空后点 G文件 选择文件")
    end
end

-- ── command handlers ──

-- 暂停/取消控制内容直接写入串口：不解析、不补行号、不加校验和，也不等待响应。
local function send_control_gcode(label, content)
    if content == "" then
        return
    end
    if not active_port then
        log("warn", label .. "指令未发送：当前没有活动串口")
        return
    end

    local ok, err = pcall(ctx.serial.write_line, active_port, content)
    if ok then
        log("info", label .. "指令已发送")
    else
        log("error", label .. "指令发送失败: " .. tostring(err))
    end
end

local function handle_send_file(payload)
    if not ensure_idle() then return end
    local sc = send_context(payload)
    local port = require_open_port(sc.target_port)
    if not port then return end

    start_task(port, function(s, task)
        task:set_status("正在检查 G-code 文件")
        -- File dialogs are asynchronous in Web. Resolve the path/opaque file
        -- handle inside the task so the same Lua source works in Native too.
        local path = resolve_file_path(sc.input)
        if not path then
            log("warn", "未选择 G-code 文件")
            return
        end
        local builder = entries_from_file(path, s, task)
        if builder then
            log("info", string.format("文件预检完成: %s (%d 条命令)",
                path, builder.content_count))
        end
        return builder
    end)
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

    start_task(port, function(s, task)
        task:set_status("正在检查输入")
        local builder = entries_from_text(sc.input, s)
        log("info", string.format("单条模式: %d 条命令", builder.content_count))
        return builder
    end)
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
        send_control_gcode("暂停", settings().pause_gcode)
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

    send_control_gcode("取消", settings().cancel_gcode)
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
    active_port = nil
    log("info", "插件已停止")
end)

migrate_legacy_config()
log("info", "G-code Sender ready")

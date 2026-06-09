-- demo.gcode-sender / main.lua
-- v0.1 — G-code 发送器
--
-- 演示 v0.3 新 API：
--   ctx.task       — 长任务（进度、暂停/恢复/取消）
--   ctx.serial.*   — write_line / read_line / write_line_and_expect
--   ctx.config     — 配置持久化 + profile
--   ctx.ui.log_*   — 插件日志面板
--   ctx.dialog.open_file / ctx.fs.read_lines — 文件选择与读取
--   hw.codec       — 内置编解码

local c = require("hw.codec")

-- ── 配置默认值 ──
local DEFAULTS = {
    port = "",
    baud_rate = 115200,
    send_delay_ms = 10,
    ack_timeout_ms = 300000,
    ok_pattern = "^ok",
    error_pattern = "^Error",
    busy_pattern = "busy",
    resend_pattern = "^Resend:",
}

-- ── 发送状态 ──
local state = {
    total_lines = 0,
    sent_lines = 0,
    cancelled = false,
    paused = false,
    current_task_id = nil,
}

-- ── UI 面板 ──
local PANEL_ID = "demo.gcode-sender.main"
local LOG_PANEL_ID = "demo.gcode-sender.log"

-- ── 初始化 ──

-- 从持久配置恢复设置
local function load_settings()
    local settings = {}
    for k, v in pairs(DEFAULTS) do
        settings[k] = ctx.config.get(k, v)
    end
    return settings
end

local function save_settings(settings)
    for k, v in pairs(settings) do
        ctx.config.set(k, v)
    end
end

local function save_profile(name)
    local t = {}
    for k, _ in pairs(DEFAULTS) do
        t[k] = ctx.config.get(k, DEFAULTS[k])
    end
    ctx.config.profile_save(name, t)
    ctx.log.info("profile '" .. name .. "' saved")
end

local function load_profile(name)
    local t = ctx.config.profile_load(name)
    if t then
        for k, v in pairs(t) do
            ctx.config.set(k, v)
        end
        ctx.log.info("profile '" .. name .. "' loaded")
        return true
    end
    return false
end

-- ── 日志辅助 ──
local function plugin_log(level, message)
    ctx.ui.log_append(LOG_PANEL_ID, { level = level, message = message })
    ctx.log.info(message)
end

-- ── 发送任务 ──
local function run_send_task(settings, lines, task)
    local total = #lines
    state.total_lines = total
    state.sent_lines = 0
    state.cancelled = false

    task:set_progress(0, total)
    task:set_status("开始发送")

    for i, raw_line in ipairs(lines) do
        if task:is_cancelled() then
            plugin_log("warn", "发送已取消 (line " .. i .. "/" .. total .. ")")
            state.cancelled = true
            return
        end

        task:wait_if_paused()

        -- 预处理：去除注释和空白
        local line = c.trim_line(raw_line)
        if line == "" or line:sub(1, 1) == ";" or line:sub(1, 1) == "(" then
            task:set_progress(i, total)
            goto continue
        end

        -- 去掉行内注释
        local semicolon = line:find(";")
        if semicolon then
            line = line:sub(1, semicolon - 1)
            line = c.trim_line(line)
            if line == "" then
                task:set_progress(i, total)
                goto continue
            end
        end

        plugin_log("info", "发送 [" .. i .. "/" .. total .. "]: " .. line)

        -- 发送并等待应答
        local resp = ctx.serial.write_line_and_expect(
            settings.port,
            line,
            {
                delimiter = "\n",
                timeout_ms = settings.ack_timeout_ms,
                patterns = {
                    { name = "ok",      pattern = settings.ok_pattern,      action = "return" },
                    { name = "error",   pattern = settings.error_pattern,   action = "return" },
                    { name = "resend",  pattern = settings.resend_pattern,  action = "return" },
                    { name = "busy",    pattern = settings.busy_pattern,    action = "continue" },
                }
            }
        )

        if resp.err then
            plugin_log("error", "无应答: " .. line .. " (" .. resp.err .. ")")
            task:set_status("无应答: 行 " .. i)
            return
        end

        local r = resp.result
        if r.name == "error" then
            plugin_log("error", "设备错误 @" .. line .. ": " .. r.line)
            task:set_status("设备错误: 行 " .. i)
            return
        end

        if r.name == "resend" then
            plugin_log("warn", "需要重发: " .. line)
            task:set_status("重发: 行 " .. i)
            -- 简单处理：重试一次
            local r2 = ctx.serial.write_line_and_expect(
                settings.port, line,
                {
                    delimiter = "\n",
                    timeout_ms = settings.ack_timeout_ms,
                    patterns = {
                        { name = "ok",    pattern = settings.ok_pattern,    action = "return" },
                        { name = "error", pattern = settings.error_pattern, action = "return" },
                    }
                }
            )
            if r2.err or not r2.result or r2.result.name ~= "ok" then
                plugin_log("error", "重发失败: " .. line)
                task:set_status("重发失败: 行 " .. i)
                return
            end
        end

        state.sent_lines = i
        task:set_progress(i, total)

        if settings.send_delay_ms > 0 then
            task:sleep_ms(settings.send_delay_ms)
        end

        ::continue::
    end

    task:set_progress(total, total)
    task:set_status("发送完成")
    plugin_log("info", "发送完成: " .. total .. " 行已发送")
end

-- ── 按钮：选择文件 ──
local function on_select_file()
    local path = ctx.dialog.open_file({
        title = "选择 G-code 文件",
        filters = {
            { name = "G-code", extensions = { "gcode", "nc", "ngc", "txt" } },
            { name = "所有文件", extensions = { "*" } },
        }
    })
    if path then
        ctx.ui.set_value(PANEL_ID, "file_path", path)
        -- 统计行数用作预览
        local count = 0
        for _ in ctx.fs.read_lines(path) do
            count = count + 1
        end
        plugin_log("info", "file loaded: " .. path .. " (" .. count .. " lines)")
    end
end

-- ── 按钮：开始发送 ──
local function on_start(values)
    local settings = load_settings()
    local path = values.file_path or ""

    if path == "" then
        plugin_log("warn", "请先选择 G-code 文件")
        return
    end

    settings.port = values.port or settings.port
    settings.baud_rate = tonumber(values.baud_rate) or settings.baud_rate
    settings.send_delay_ms = tonumber(values.send_delay_ms) or settings.send_delay_ms

    save_settings(settings)

    -- 打开串口
    local ok, err = pcall(ctx.serial.open, {
        port_name = settings.port,
        baud_rate = settings.baud_rate,
    })
    if not ok then
        plugin_log("error", "无法打开串口 " .. settings.port .. ": " .. tostring(err))
        return
    end

    -- 清空接收缓冲
    ctx.serial.flush_rx(settings.port)

    -- 读取文件行
    local lines = {}
    for line in ctx.fs.read_lines(path) do
        table.insert(lines, line)
    end

    if #lines == 0 then
        plugin_log("warn", "文件为空")
        return
    end

    plugin_log("info", "开始发送 " .. #lines .. " 行到 " .. settings.port)

    -- 启动发送任务
    local task = ctx.task.start({
        id = "gcode.send",
        title = "发送 G-code",
        cancellable = true,
        pausable = true,
    }, function(t)
        run_send_task(settings, lines, t)
    end)

    state.current_task_id = task.id
    ctx.ui.set_enabled(PANEL_ID, "btn_start", false)
    ctx.ui.set_enabled(PANEL_ID, "btn_pause", true)
    ctx.ui.set_enabled(PANEL_ID, "btn_cancel", true)
end

-- ── 按钮：暂停 / 恢复 ──
local function on_pause()
    if state.paused then
        ctx.task.resume("gcode.send")
        state.paused = false
        ctx.ui.set_value(PANEL_ID, "btn_pause", "暂停")
        plugin_log("info", "发送已恢复")
    else
        ctx.task.pause("gcode.send")
        state.paused = true
        ctx.ui.set_value(PANEL_ID, "btn_pause", "恢复")
        plugin_log("info", "发送已暂停")
    end
end

-- ── 按钮：取消 ──
local function on_cancel()
    ctx.task.cancel("gcode.send")
    state.cancelled = true
    ctx.ui.set_enabled(PANEL_ID, "btn_start", true)
    ctx.ui.set_enabled(PANEL_ID, "btn_pause", false)
    ctx.ui.set_enabled(PANEL_ID, "btn_cancel", false)
    plugin_log("warn", "发送取消请求")
end

-- ── 按钮：保存 profile ──
local function on_save_profile(values)
    local name = values.profile_name or ""
    if name == "" then
        plugin_log("warn", "请输入 profile 名称")
        return
    end
    -- 先保存当前设置
    local settings = {
        port = values.port or "",
        baud_rate = tonumber(values.baud_rate) or 115200,
        send_delay_ms = tonumber(values.send_delay_ms) or 10,
    }
    save_settings(settings)
    save_profile(name)
end

-- ── 按钮：加载 profile ──
local function on_load_profile(values)
    local name = values.profile_name or ""
    if name == "" then
        plugin_log("warn", "请输入 profile 名称")
        return
    end
    if load_profile(name) then
        -- 更新 UI 字段
        local settings = load_settings()
        ctx.ui.set_value(PANEL_ID, "port", settings.port)
        ctx.ui.set_value(PANEL_ID, "baud_rate", tostring(settings.baud_rate))
        ctx.ui.set_value(PANEL_ID, "send_delay_ms", tostring(settings.send_delay_ms))
    else
        plugin_log("warn", "profile '" .. name .. "' 不存在")
    end
end

-- ── 初始化 ──
local function init()
    -- 创建日志面板
    ctx.ui.create_log({
        id = LOG_PANEL_ID,
        title = "发送日志",
        max_entries = 5000,
    })

    -- 创建主面板
    local settings = load_settings()

    ctx.ui.create_form({
        id = PANEL_ID,
        title = "G-code 发送器",
        fields = {
            { id = "file_path", kind = "File",       title = "G-code 文件", filters = { { name = "G-code", extensions = { "gcode", "nc", "ngc", "txt" } } } },
            { id = "port",      kind = "TextArea",   title = "串口号",      value = settings.port, rows = 1 },
            { id = "baud_rate", kind = "TextArea",   title = "波特率",      value = tostring(settings.baud_rate), rows = 1 },
            { id = "send_delay_ms", kind = "TextArea", title = "发送间隔(ms)", value = tostring(settings.send_delay_ms), rows = 1 },
            { id = "profile_name", kind = "TextArea", title = "Profile 名", value = "", rows = 1 },
            { kind = "Separator" },
            { id = "btn_select",    kind = "Button", title = "选择文件",   action = "gcode.select_file" },
            { id = "btn_start",     kind = "Button", title = "开始发送",   action = "gcode.start" },
            { id = "btn_pause",     kind = "Button", title = "暂停",       action = "gcode.pause", enabled = false },
            { id = "btn_cancel",    kind = "Button", title = "取消",       action = "gcode.cancel", enabled = false },
            { kind = "Separator" },
            { id = "btn_save_profile",   kind = "Button", title = "保存 Profile", action = "gcode.save_profile" },
            { id = "btn_load_profile",   kind = "Button", title = "加载 Profile", action = "gcode.load_profile" },
            { kind = "Label", text = "提示：选择串口和 G-code 文件后点击开始发送。暂停/取消可在发送中途控制。" },
            { id = "progress", kind = "Progress", title = "进度" },
            { id = "status",   kind = "Status",   title = "状态" },
        }
    })

    -- 更新进度和状态的初始值
    ctx.ui.set_value(PANEL_ID, "progress", { current = 0, total = 0 })
    ctx.ui.set_value(PANEL_ID, "status", "就绪")

    plugin_log("info", "G-code Sender v0.1 initialized")
end

-- ── 事件处理 ──

-- 处理表单按钮点击
ctx.bus.on("ui.form.action", function(event)
    local p = event.payload
    if p.panel_id ~= PANEL_ID then return end

    if p.action == "gcode.select_file" then
        on_select_file()
    elseif p.action == "gcode.start" then
        on_start(p.values or {})
    elseif p.action == "gcode.pause" then
        on_pause()
    elseif p.action == "gcode.cancel" then
        on_cancel()
    elseif p.action == "gcode.save_profile" then
        on_save_profile(p.values or {})
    elseif p.action == "gcode.load_profile" then
        on_load_profile(p.values or {})
    end
end)

-- 定期更新进度
ctx.timer.every(500, function()
    if state.current_task_id then
        local tasks = ctx.task.list()
        for _, t in ipairs(tasks) do
            if t.id == "gcode.send" then
                ctx.ui.set_value(PANEL_ID, "progress", {
                    current = t.progress_current,
                    total = t.progress_total,
                })
                ctx.ui.set_value(PANEL_ID, "status", t.status)
                if t.finished then
                    ctx.ui.set_enabled(PANEL_ID, "btn_start", true)
                    ctx.ui.set_enabled(PANEL_ID, "btn_pause", false)
                    ctx.ui.set_enabled(PANEL_ID, "btn_cancel", false)
                    state.current_task_id = nil
                    if t.error and t.error ~= "" then
                        plugin_log("error", "task error: " .. t.error)
                    end
                end
                break
            end
        end
    end
end)

-- ── 启动 ──
init()

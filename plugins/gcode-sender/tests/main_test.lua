package.preload["hw.codec"] = function()
    return { xor8 = function() return 0 end }
end

local callbacks = {}
local current_task
local writes = {}
local config_values = { default_setup_gcode = "M92 X40 Y40 Z2.5 E7.53" }
local file_contents = {}
local followup_lines = {}
local logs = {}
local responder
local serial_status_calls = 0
local clock = 0

local function ok_response(line)
    return { result = { name = "ok", line = line or "ok" } }
end

local function result_response(name, line)
    return { result = { name = name, line = line } }
end

ctx = {
    plugin = { id = "gcode-sender" },
    log = {},
    config = {
        get = function(key, default)
            local value = config_values[key]
            if value == nil then return default end
            return value
        end,
        set = function(key, value)
            config_values[key] = value
        end,
    },
    commands = {
        register = function(command, callback) callbacks[command] = callback end,
    },
    ui = {
        set_status = function(message) ctx.last_ui_status = message end,
        set_contribution_value = function(_, value) ctx.last_progress = value end,
    },
    serial = {
        status_port = function()
            serial_status_calls = serial_status_calls + 1
            return { open = true }
        end,
        flush_rx = function() end,
        write_line_and_expect = function(_, wire, options)
            writes[#writes + 1] = wire
            ctx.last_expect_options = options
            return responder(wire, #writes)
        end,
        read_line = function()
            if #followup_lines == 0 then return { err = "timeout" } end
            return { line = table.remove(followup_lines, 1) }
        end,
    },
    fs = {
        read_lines_stream = function(path)
            local lines = file_contents[path]
            if not lines then error("file not found: " .. path) end
            local index = 0
            return function()
                index = index + 1
                return lines[index]
            end
        end,
    },
    dialog = {},
    task = {},
    now_ms = function()
        clock = clock + 10
        return clock
    end,
}

for _, level in ipairs({ "debug", "info", "warn", "error" }) do
    ctx.log[level] = function(message)
        logs[#logs + 1] = { level = level, message = tostring(message) }
    end
end

function on_disable(callback)
    ctx.disable_callback = callback
end

function ctx.task.list()
    if current_task then return { current_task } end
    return {}
end

function ctx.task.start(options, callback)
    local state = {
        id = options.id,
        paused = false,
        cancelled = false,
        finished = false,
        status = "",
    }
    current_task = state
    local task = {}
    function task:is_cancelled() return state.cancelled end
    function task:set_status(value) state.status = value end
    function task:set_progress(value, total)
        state.progress = value
        state.progress_total = total
    end
    function task:sleep_ms() end
    function task:wait_if_paused() end
    callback(task)
    state.finished = true
end

function ctx.task.pause()
    current_task.paused = true
    ctx.last_task_action = "pause"
end

function ctx.task.resume()
    current_task.paused = false
    ctx.last_task_action = "resume"
end

function ctx.task.cancel()
    current_task.cancelled = true
    current_task.paused = false
end

dofile((arg[0]:gsub("tests[\\/]main_test.lua$", "main.lua")))
assert(config_values.default_setup_gcode == "", "legacy setup default must be migrated away")

local function reset()
    current_task = nil
    writes = {}
    config_values = {}
    file_contents = {}
    followup_lines = {}
    logs = {}
    responder = function() return ok_response() end
    serial_status_calls = 0
    clock = 0
    ctx.last_ui_status = nil
    ctx.last_task_action = nil
    ctx.last_expect_options = nil
end

local function send_single(input)
    callbacks["gcode-sender.send_single"]({
        context = {
            send = { input = input, target_port = "COM1", target_port_open = true },
        },
    })
end

local function send_file(path)
    callbacks["gcode-sender.send_file"]({
        context = {
            send = { input = path, target_port = "COM1", target_port_open = true },
        },
    })
end

-- 已有行号/校验和会被移除，M110 会被跳过；默认不改写作业结尾。
reset()
send_single("N42 G1 X1*99 ; old checksum\nM110 N0\nM30")
assert(#writes == 3, "expected M110 sync plus two G-code lines")
assert(writes[2]:match("^N1 G1 X1%*"), writes[2])
assert(writes[3]:match("^N2 M30%*"), writes[3])

-- 初始化命令必须同时用于文件和单条模式，默认初始化为空。
reset()
config_values.default_setup_gcode = "G21"
file_contents["job.gcode"] = { "; comment", "G1 X1" }
send_file("job.gcode")
assert(#writes == 3, "expected sync, setup and file command")
assert(writes[2]:match("^N1 G21%*"), writes[2])
assert(writes[3]:match("^N2 G1 X1%*"), writes[3])

reset()
send_single("G1 X1")
assert(#writes == 2, "default setup and M2 must both be empty/off")

-- M2 只有显式启用时才追加。
reset()
config_values.append_program_end = true
send_single("G1 X1")
assert(#writes == 3, "expected sync, command and opt-in M2")
assert(writes[3]:match("^N2 M2%*"), writes[3])

-- 任一最终发送行超长都必须阻止整个作业，不能静默跳过。
reset()
config_values.max_marlin_line_bytes = 32
file_contents["too-long.gcode"] = {
    "; heading",
    "G1 X123456789012345678901234567890",
    "G1 X1",
}
send_file("too-long.gcode")
assert(#writes == 0, "preflight failure must send nothing")
assert(current_task.status == "存在超长 G-code，未发送", current_task.status)
local saw_source_line = false
for _, item in ipairs(logs) do
    if item.message:find("文件第 2 行", 1, true) then saw_source_line = true end
end
assert(saw_source_line, "preflight error must report the original file line")

-- ACK 超时只执行配置数量的额外重试，不会无限循环。
reset()
config_values.max_retries = 2
responder = function(wire)
    if wire:match("^N0 M110") then return ok_response() end
    return { err = "timeout" }
end
send_single("G1 X1")
assert(#writes == 4, "expected sync plus initial attempt and two retries")
assert(current_task.status == "ACK 超时，已停止", current_task.status)

-- 等待 ACK 时取消应立即结束，不能被当作超时再次发送。
reset()
responder = function(wire)
    if wire:match("^N0 M110") then return ok_response() end
    current_task.cancelled = true
    return { err = "cancelled" }
end
send_single("G1 X1")
assert(#writes == 2, "cancelled command must not be retried")
assert(current_task.status == "已取消", current_task.status)

-- Unknown command 是确定的设备错误，不能伪装成 ACK 超时继续重试。
reset()
config_values.error_followup_ms = 100
responder = function(wire)
    if wire:match("^N0 M110") then return ok_response() end
    return result_response("error", "Error:Unknown command: G999")
end
send_single("G999")
assert(#writes == 2, "unknown command must stop without retries")
assert(current_task.status == "设备错误，已停止", current_task.status)

-- M110 阶段的确定错误应立即停止并保留设备错误，不应反复重试成“无响应”。
reset()
responder = function()
    return result_response("error", "Error:Printer halted. kill() called!")
end
send_single("G1 X1")
assert(#writes == 1, "fatal M110 error must not be retried")
assert(current_task.status == "行号同步失败", current_task.status)

-- Resend:N 能跳回指定行；末行 ACK 丢失后的 N+1 也视为设备已接收。
reset()
local requested_resend = false
responder = function(wire)
    if wire:match("^N0 M110") then return ok_response() end
    if wire:match("^N2 ") and not requested_resend then
        requested_resend = true
        return result_response("resend", "Resend: N1")
    end
    return ok_response()
end
send_single("G1 X1\nG1 X2")
assert(#writes == 5, "expected sync, N1, N2, then N1/N2 resend")

reset()
responder = function(wire, write_count)
    if wire:match("^N0 M110") then return ok_response() end
    if write_count == 2 then return result_response("resend", "Resend: N2") end
    return ok_response()
end
send_single("G1 X1")
assert(#writes == 2, "N(total+1) means the final line was already accepted")
assert(current_task.status == "发送完成", current_task.status)

-- 带时间前缀的 error 后续 Resend 也必须识别。
reset()
local first_command = true
config_values.error_followup_ms = 100
followup_lines = { "(0.125)Resend: N1" }
responder = function(wire)
    if wire:match("^N0 M110") then return ok_response() end
    if first_command then
        first_command = false
        return result_response("error", "(0.100)Error:Line Number is not Last Line Number+1")
    end
    return ok_response()
end
send_single("G1 X1")
assert(#writes == 3, "prefixed Resend must retry the requested line")

-- busy/keepalive 必须启用空闲超时刷新。
assert(ctx.last_expect_options.continue_resets_timeout == true)

-- 暂停按钮必须以宿主任务状态为准，而不是插件私有状态。
reset()
current_task = { id = "gcode-sender.print", paused = true, cancelled = false, finished = false }
callbacks["gcode-sender.pause"]({})
assert(ctx.last_task_action == "resume")
callbacks["gcode-sender.pause"]({})
assert(ctx.last_task_action == "pause")

-- 任务忙时应在检查串口、打开文件之前直接拒绝。
reset()
current_task = { id = "gcode-sender.print", paused = false, cancelled = false, finished = false }
send_single("G1 X1")
assert(serial_status_calls == 0, "busy task should be rejected before touching the port")

print("gcode-sender tests passed")

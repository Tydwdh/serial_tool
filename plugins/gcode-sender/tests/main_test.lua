package.preload["hw.codec"] = function()
    return { xor8 = function() return 0 end }
end

local callbacks = {}
local current_task
local writes = {}
local config_values = {}
local responder
local serial_status_calls = 0
local clock = 0

local function ok_response()
    return { result = { name = "ok", line = "ok" } }
end

ctx = {
    plugin = { id = "gcode-sender" },
    log = {
        debug = function() end,
        info = function() end,
        warn = function() end,
        error = function() end,
    },
    config = {
        get = function(key, default)
            local value = config_values[key]
            if value == nil then return default end
            return value
        end,
    },
    commands = {
        register = function(command, callback) callbacks[command] = callback end,
    },
    ui = {
        set_status = function(message) ctx.last_ui_status = message end,
        set_contribution_value = function() end,
    },
    serial = {
        status_port = function()
            serial_status_calls = serial_status_calls + 1
            return { open = true }
        end,
        flush_rx = function() end,
        write_line_and_expect = function(_, wire)
            writes[#writes + 1] = wire
            return responder(wire, #writes)
        end,
        read_line = function() return { err = "timeout" } end,
    },
    fs = {},
    dialog = {},
    task = {},
    now_ms = function()
        clock = clock + 10
        return clock
    end,
}

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
    function task:set_progress() end
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

local function reset()
    current_task = nil
    writes = {}
    config_values = {}
    responder = function() return ok_response() end
    serial_status_calls = 0
    ctx.last_ui_status = nil
    ctx.last_task_action = nil
end

local function send_single(input)
    callbacks["gcode-sender.send_single"]({
        context = {
            send = { input = input, target_port = "COM1", target_port_open = true },
        },
    })
end

-- 已有行号/校验和会被移除，M110 会被跳过，M30 不会再追加 M2。
reset()
config_values.default_setup_gcode = ""
send_single("N42 G1 X1*99 ; old checksum\nM110 N0\nM30")
assert(#writes == 3, "expected M110 sync plus two G-code lines")
assert(writes[2]:match("^N1 G1 X1%*"), writes[2])
assert(writes[3]:match("^N2 M30%*"), writes[3])

-- 最终线长包含行号和校验和；超长行应被跳过而不是发给设备。
reset()
config_values.default_setup_gcode = ""
config_values.max_marlin_line_bytes = 32
send_single("G1 X123456789012345678901234567890\nG1 X1")
assert(#writes == 3, "expected sync, short command and M2")
assert(not writes[2]:find("1234567890", 1, true), "overlong wire line was sent")

-- ACK 超时只执行配置数量的额外重试，不会无限循环。
reset()
config_values.default_setup_gcode = ""
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
config_values.default_setup_gcode = ""
responder = function(wire)
    if wire:match("^N0 M110") then return ok_response() end
    current_task.cancelled = true
    return { err = "cancelled" }
end
send_single("G1 X1")
assert(#writes == 2, "cancelled command must not be retried")
assert(current_task.status == "已取消", current_task.status)

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

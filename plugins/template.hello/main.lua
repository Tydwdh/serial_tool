-- Hello 插件模板 (v0.2)
--
-- 功能：
--   1. 输出日志
--   2. 创建动态表单（text/select/checkbox）
--   3. 监听表单变更并持久化
--   4. 演示 hw.utils 和 hw.codec
--   5. 停用时清理面板
--
-- 复制本目录开发新插件时，请修改：
--   plugin.json 的 id/name/version
--   面板 id，避免和其他插件冲突

local u = require("hw.utils")
local c = require("hw.codec")

local PANEL_ID = "template.hello.form"

ctx.log.info("Hello 插件已启动: " .. tostring(ctx.plugin.id))
ctx.log.info("hw.utils demo: format_size(4096) = " .. u.format_size(4096))
ctx.log.info("hw.codec demo: to_hex('AB') = " .. c.to_hex("AB"))

ctx.ui.create_form({
    id = PANEL_ID,
    title = "Hello 参数",
    auto_apply = true,
    fields = {
        {
            id = "message",
            label = "消息",
            kind = "text",
            default = ctx.session.get("message") or "hello hardware workbench"
        },
        {
            id = "level",
            label = "日志级别",
            kind = "select",
            default = ctx.session.get("level") or "info",
            options = {
                { label = "Info", value = "info" },
                { label = "Warn", value = "warn" },
                { label = "Error", value = "error" }
            }
        },
        {
            id = "enabled",
            label = "启用输出",
            kind = "checkbox",
            default = true
        }
    }
})

local function log_by_level(level, message)
    if level == "warn" then
        ctx.log.warn(message)
    elseif level == "error" then
        ctx.log.error(message)
    else
        ctx.log.info(message)
    end
end

ctx.bus.on("ui.form.changed", function(event)
    if not event.payload or event.payload.panel_id ~= PANEL_ID then
        return
    end

    local values = event.payload.values or {}
    local message = tostring(values.message or "")
    local level = tostring(values.level or "info")
    local enabled = values.enabled

    ctx.session.set("message", message)
    ctx.session.set("level", level)

    if enabled then
        log_by_level(level, "Hello 表单变更: " .. message)
    end
end)

on_disable(function()
    ctx.ui.remove_panel(PANEL_ID)
    ctx.log.info("Hello 插件已停止")
end)

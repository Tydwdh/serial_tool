-- Hello 插件模板
--
-- 功能：
--   1. 输出日志
--   2. 创建一个动态表单
--   3. 监听表单变更
--   4. 停用时清理面板
--
-- 复制本目录开发新插件时，请修改：
--   plugin.json 的 id/name/version
--   面板 id，避免和其他插件冲突

local PANEL_ID = "template.hello.form"

ctx.log.info("Hello 插件已启动: " .. tostring(ctx.plugin.id))

ctx.ui.create_form({
    id = PANEL_ID,
    title = "Hello 参数",
    auto_apply = true,
    fields = {
        {
            id = "message",
            label = "消息",
            kind = "text",
            default = ctx.storage.get("message") or "hello hardware workbench"
        },
        {
            id = "level",
            label = "日志级别",
            kind = "select",
            default = ctx.storage.get("level") or "info",
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

    ctx.storage.set("message", message)
    ctx.storage.set("level", level)

    if enabled then
        log_by_level(level, "Hello 表单变更: " .. message)
    end
end)

on_disable(function()
    ctx.ui.remove_panel(PANEL_ID)
    ctx.log.info("Hello 插件已停止")
end)

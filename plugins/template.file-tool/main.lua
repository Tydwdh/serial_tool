-- 文件工具 (Template)
-- 演示 v0.2 新增的 file / textarea / button / progress / status 字段类型
-- 以及 ctx.dialog.open_file / ctx.fs.read_text / ctx.ui.set_value

local selected_file = ""

ctx.ui.create_form({
    id = "file-tool-panel",
    title = "文件工具",
    auto_apply = true,
    fields = {
        {
            id = "file_path",
            label = "文件路径",
            kind = "file",
            filters = {
                { name = "文本文件", extensions = { "txt", "log", "csv", "json" } },
                { name = "所有文件", extensions = { "*" } }
            }
        },
        {
            id = "file_content",
            label = "文件内容",
            kind = "textarea",
            rows = 10,
            default = ""
        },
        {
            id = "load_btn",
            label = "加载文件",
            kind = "button",
            variant = "primary"
        },
        { kind = "separator" },
        {
            id = "progress",
            label = "进度",
            kind = "progress",
            default = 0
        },
        {
            id = "status",
            label = "状态",
            kind = "status",
            default = {
                text = "就绪",
                level = "idle"
            }
        },
        {
            id = "stats",
            label = "统计信息",
            kind = "label",
            text = ""
        }
    }
})

-- 点击按钮后加载文件
ctx.bus.on("ui.form.action", function(event)
    if not event.payload or event.payload.panel_id ~= "file-tool-panel" then
        return
    end
    if event.payload.field_id == "load_btn" then
        load_selected_file()
    end
end)

function load_selected_file()
    local path = selected_file
    if path == nil or path == "" then
        ctx.ui.set_value("file-tool-panel", "status", {
            text = "请先选择文件",
            level = "warn"
        })
        return
    end

    ctx.ui.set_value("file-tool-panel", "status", {
        text = "读取中...",
        level = "running"
    })
    ctx.ui.set_value("file-tool-panel", "progress", 10)

    local ok, content = pcall(function()
        return ctx.fs.read_text(path)
    end)

    ctx.ui.set_value("file-tool-panel", "progress", 80)

    if not ok then
        ctx.ui.set_value("file-tool-panel", "status", {
            text = "读取失败: " .. tostring(content),
            level = "error"
        })
        ctx.ui.set_value("file-tool-panel", "progress", 0)
        return
    end

    ctx.ui.set_value("file-tool-panel", "file_content", content)

    -- 统计
    local lines = 0
    for _ in content:gmatch("\n") do
        lines = lines + 1
    end
    local chars = #content

    ctx.ui.set_value("file-tool-panel", "progress", 100)
    ctx.ui.set_value("file-tool-panel", "status", {
        text = "加载完成",
        level = "success"
    })
    ctx.ui.set_value("file-tool-panel", "stats", string.format(
        "共 %d 行, %d 字符", lines, chars
    ))

    ctx.log.info(string.format("已加载: %s (%d 行)", path, lines))
end

-- 表单变化时记录选中的文件路径
ctx.bus.on("ui.form.changed", function(event)
    if not event.payload or event.payload.panel_id ~= "file-tool-panel" then
        return
    end
    if event.payload.values and event.payload.values.file_path then
        selected_file = event.payload.values.file_path
    end
end)

ctx.log.info("文件工具插件已就绪")

-- 文件工具 (Template)
-- 演示 v0.2 新增的 file / textarea / button / progress / status 字段类型
-- 以及 ctx.dialog.open_file / ctx.fs.read_text / ctx.ui.set_value

-- 点击按钮后加载文件（从 event.payload.values 读取表单当前状态）
ctx.bus.on("ui.form.action", function(event)
    if not event.payload or event.payload.panel_id ~= "file-tool-panel" then
        return
    end
    if event.payload.field_id == "load_btn" then
        local values = event.payload.values or {}
        load_selected_file(values.file_path)
    end
end)

function load_selected_file(path)
    if path == nil or path == "" then
        ctx.ui.set_value("file-tool-panel", "status", {
            text = "请先选择文件",
            level = "warn"
        })
        return
    end

    ctx.ui.set_values("file-tool-panel", {
        status = { text = "读取中...", level = "running" },
        progress = 10
    })

    local ok, content = pcall(function()
        return ctx.fs.read_text(path)
    end)

    ctx.ui.set_value("file-tool-panel", "progress", 80)

    if not ok then
        ctx.ui.set_values("file-tool-panel", {
            status = { text = "读取失败: " .. tostring(content), level = "error" },
            progress = 0
        })
        return
    end

    ctx.ui.set_value("file-tool-panel", "file_content", content)

    -- 统计
    local lines = 0
    for _ in content:gmatch("\n") do
        lines = lines + 1
    end
    local chars = #content

    ctx.ui.set_values("file-tool-panel", {
        progress = 100,
        status = { text = "加载完成", level = "success" },
        stats = string.format("共 %d 行, %d 字符", lines, chars)
    })

    ctx.log.info(string.format("已加载: %s (%d 行)", path, lines))
end

-- 表单变化时记录选中的文件路径
ctx.log.info("文件工具插件已就绪")

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    // 验证非 UI 依赖在 Slint 壳中可被复用（不实际开串口，仅证明打通）
    let _bus = tool_databus::DataBus::new();
    let _ = tool_core::topics::SERIAL_RX;
    let _ = tool_core::now_timestamp_ms();

    let app = AppWindow::new()?;

    let weak = app.as_weak();
    app.on_tx_send(move |text| {
        if let Some(handle) = weak.upgrade() {
            let trimmed = text.trim().to_string();
            let count = handle.get_launch_count() + 1;
            handle.set_launch_count(count);
            if trimmed.is_empty() {
                handle.set_status_text(
                    format!("第 {count} 次触发：输入为空（POC 未真发）").into(),
                );
            } else {
                // 展示“已接管输入”的反馈，后续在此调用 transport/databus 真发
                handle.set_status_text(
                    format!("第 {count} 次发送(模拟)：{trimmed}").into(),
                );
                let preview = handle.get_rx_preview();
                handle.set_rx_preview(
                    format!("{preview}\n[TX 模拟 {count}] {trimmed}").into(),
                );
            }
            handle.set_log_preview(format!("模拟发送 #{count}").into());
            let _ = &text;
        }
    });

    let weak2 = app.as_weak();
    app.on_open_config_folder(move || {
        // 复用 app/crate 已有逻辑：配置目录
        let base = dirs_next::config_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let dir = base.join("HardwareWorkbench");
        let msg = if dir.exists() {
            format!("配置目录：{}", dir.display())
        } else {
            format!("配置目录（尚未创建）：{}", dir.display())
        };
        if let Some(h) = weak2.upgrade() {
            h.set_status_text(msg.into());
        }
        // 非阻塞尝试打开目录（失败仅提示）
        let _ = open::that(&dir);
    });

    app.run()
}

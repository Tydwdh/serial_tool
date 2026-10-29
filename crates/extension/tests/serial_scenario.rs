//! 仓库内场景文件的自动回归：`plugins/template.serial-chart/tests/serial.scenario.json`。
//!
//! 这条场景此前只能手工跑（`cargo run -p tool-extension --example plugin_scenario`），
//! 模板插件坏了没人会知道。运行器本身不碰硬件：内存串口 + 真实
//! PluginManager / Lua 运行时 / DataBus，用法与 `docs/plugins/scenarios.md` 一致。

use std::path::Path;

use tool_extension::SerialPluginScenarioRunner;
use tool_testing::{SerialPluginScenario, TestStatus};

#[test]
fn bundled_serial_chart_scenario_passes() {
    let plugins_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins");
    let scenario_path = plugins_root.join("template.serial-chart/tests/serial.scenario.json");
    let source = std::fs::read_to_string(&scenario_path)
        .unwrap_or_else(|error| panic!("读取 {} 失败：{error}", scenario_path.display()));
    let scenario: SerialPluginScenario =
        serde_json::from_str(&source).expect("场景文件必须是合法 JSON");

    let report = SerialPluginScenarioRunner::new()
        .run(&plugins_root, &scenario)
        .expect("场景运行器不应返回错误");

    let failed: Vec<&str> = report
        .cases
        .iter()
        .filter(|case| case.status != TestStatus::Passed)
        .map(|case| case.name.as_str())
        .collect();
    assert!(
        failed.is_empty(),
        "场景用例未通过：{failed:?}（完整报告：{report:#?}）"
    );
}

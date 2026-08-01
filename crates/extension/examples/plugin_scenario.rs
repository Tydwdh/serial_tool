use std::path::PathBuf;
use tool_extension::SerialPluginScenarioRunner;
use tool_testing::{SerialPluginScenario, TestStatus};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let plugins_root = PathBuf::from(
        args.next()
            .ok_or("用法: plugin_scenario <plugins-root> <scenario.json>")?,
    );
    let scenario_path = PathBuf::from(args.next().ok_or("缺少 scenario.json")?);
    let scenario: SerialPluginScenario =
        serde_json::from_str(&std::fs::read_to_string(&scenario_path)?)?;
    let report = SerialPluginScenarioRunner::new().run(&plugins_root, &scenario)?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    if report
        .cases
        .iter()
        .any(|case| case.status != TestStatus::Passed)
    {
        std::process::exit(1);
    }
    Ok(())
}

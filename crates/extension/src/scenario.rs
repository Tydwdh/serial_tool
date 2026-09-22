use crate::PluginManager;
use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};
use tool_core::{Direction, Event, Payload, now_timestamp_ms, topics};
use tool_databus::{DataBus, Subscription, TopicFilter};
use tool_testing::{
    SerialPluginScenario, SerialScenarioStep, TestCaseResult, TestPacketLog, TestRunReport,
    TestStatus,
};
use tool_transport::{TransportManager, serial_topics};

/// 使用真实 PluginManager、Lua runtime 和内存串口执行 JSON 场景。
pub struct SerialPluginScenarioRunner {
    bus: DataBus,
    transport: TransportManager,
    manager: PluginManager,
    events: Subscription,
    backlog: VecDeque<Event>,
    observed: Vec<Event>,
}

impl SerialPluginScenarioRunner {
    pub fn new() -> Self {
        let bus = DataBus::new();
        let events = bus.subscribe(TopicFilter::All);
        let transport = TransportManager::new(bus.clone());
        let manager = PluginManager::new(bus.clone(), transport.clone());
        Self {
            bus,
            transport,
            manager,
            events,
            backlog: VecDeque::new(),
            observed: Vec::new(),
        }
    }

    pub fn run(
        &mut self,
        plugins_root: &Path,
        scenario: &SerialPluginScenario,
    ) -> Result<TestRunReport, String> {
        self.backlog.clear();
        self.observed.clear();
        let _ = self.events.drain();
        self.manager
            .discover_roots([plugins_root.to_path_buf()])
            .map_err(|error| error.to_string())?;
        let virtual_port = self
            .transport
            .open_virtual_serial(scenario.port.clone())
            .map_err(|error| error.to_string())?;
        self.manager
            .enable(&scenario.plugin_id)
            .map_err(|error| error.to_string())?;
        self.pump(Duration::from_millis(20));

        let started_ms = now_timestamp_ms();
        let started = Instant::now();
        let mut logs = Vec::new();
        let mut assertions = 0;
        let mut failure = None;

        for (index, step) in scenario.steps.iter().enumerate() {
            let result = match step {
                SerialScenarioStep::Rx { data } => {
                    data.bytes().map(|bytes| virtual_port.inject_rx(bytes))
                }
                SerialScenarioStep::Execute {
                    command,
                    input,
                    payload,
                } => {
                    self.execute(&scenario.plugin_id, command, input, payload.clone());
                    Ok(())
                }
                SerialScenarioStep::Cancel { command } => {
                    self.execute(&scenario.plugin_id, command, "", serde_json::Value::Null);
                    Ok(())
                }
                SerialScenarioStep::Wait { ms } => {
                    self.pump(Duration::from_millis(*ms));
                    Ok(())
                }
                SerialScenarioStep::ExpectTx { data, timeout_ms } => {
                    assertions += 1;
                    let expected = data.bytes()?;
                    self.expect_event(timeout_ms.unwrap_or(scenario.timeout_ms), |event| {
                        event.topic == serial_topics::SERIAL_TX
                            && event.meta_str("port") == Some(scenario.port.as_str())
                            && matches!(&event.payload, Payload::Bytes(bytes) if bytes == &expected)
                    })
                    .map(|_| ())
                    .ok_or_else(|| format!("未收到预期 TX: {}", String::from_utf8_lossy(&expected)))
                }
                SerialScenarioStep::ExpectNoTx { timeout_ms } => {
                    assertions += 1;
                    let unexpected_tx =
                        self.expect_event(timeout_ms.unwrap_or(scenario.timeout_ms), |event| {
                            event.topic == serial_topics::SERIAL_TX
                                && event.meta_str("port") == Some(scenario.port.as_str())
                        });
                    if unexpected_tx.is_some() {
                        Err("等待窗口内出现了意外 TX".to_owned())
                    } else {
                        Ok(())
                    }
                }
                SerialScenarioStep::ExpectEvent {
                    topic,
                    payload,
                    timeout_ms,
                } => {
                    assertions += 1;
                    self.expect_event(timeout_ms.unwrap_or(scenario.timeout_ms), |event| {
                        event.topic == *topic && payload.as_ref().is_none_or(|expected| {
                            matches!(&event.payload, Payload::Json(actual) if json_contains(actual, expected))
                        })
                    })
                    .map(|_| ())
                    .ok_or_else(|| format!("未收到预期事件: {topic}"))
                }
            };
            match result {
                Ok(()) => logs.push(format!("step {} passed", index + 1)),
                Err(error) => {
                    failure = Some(format!("step {}: {error}", index + 1));
                    break;
                }
            }
            self.pump(Duration::from_millis(5));
        }

        let _ = self.manager.disable(&scenario.plugin_id);
        self.pump(Duration::from_millis(20));
        self.transport.close_port(&scenario.port);
        let status = if failure.is_some() {
            TestStatus::Failed
        } else {
            TestStatus::Passed
        };
        let finished_ms = now_timestamp_ms();
        let raw_packets = self
            .observed
            .iter()
            .filter(|event| {
                event.topic == serial_topics::SERIAL_RX || event.topic == serial_topics::SERIAL_TX
            })
            .map(packet_log)
            .collect();
        Ok(TestRunReport {
            run_id: format!("scenario-{}-{started_ms}", scenario.plugin_id),
            source: "serial-plugin-scenario".to_owned(),
            script_name: scenario.name.clone(),
            started_ms,
            finished_ms,
            cases: vec![TestCaseResult {
                name: scenario.name.clone(),
                status,
                duration_ms: started.elapsed().as_millis() as u64,
                logs,
                assertions,
                error: failure,
                raw_packets,
            }],
        })
    }

    fn execute(&self, plugin_id: &str, command: &str, input: &str, payload: serde_json::Value) {
        let target_port = self
            .transport
            .open_ports()
            .first()
            .cloned()
            .unwrap_or_default();
        self.bus.publish(Event::new(
            topics::PLUGIN_COMMAND_EXECUTE,
            "scenario",
            Direction::Internal,
            Payload::Json(serde_json::json!({
                "plugin_id": plugin_id,
                "command": command,
                "context": {
                    "send": {
                        "input": input,
                        "target_port": target_port,
                        "target_port_open": true
                    }
                },
                "payload": payload
            })),
        ));
    }

    fn pump(&mut self, duration: Duration) {
        let deadline = Instant::now() + duration;
        loop {
            self.manager.process_pending();
            self.collect_events();
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn expect_event(
        &mut self,
        timeout_ms: u64,
        predicate: impl Fn(&Event) -> bool,
    ) -> Option<Event> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            if let Some(index) = self.backlog.iter().position(&predicate) {
                return self.backlog.remove(index);
            }
            self.manager.process_pending();
            self.collect_events();
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn collect_events(&mut self) {
        let events = self.events.drain();
        self.observed.extend(events.iter().cloned());
        self.backlog.extend(events);
    }
}

impl Default for SerialPluginScenarioRunner {
    fn default() -> Self {
        Self::new()
    }
}

fn packet_log(event: &Event) -> TestPacketLog {
    TestPacketLog {
        id: event.id,
        timestamp_ms: event.timestamp_ms,
        topic: event.topic.clone(),
        direction: event.direction,
        payload_text: event.payload.text_lossy(),
        payload_hex: payload_hex(&event.payload),
    }
}

/// 非字节负载没有十六进制形式，记为空串。
fn payload_hex(payload: &Payload) -> String {
    payload
        .as_bytes()
        .unwrap_or_default()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn json_contains(actual: &serde_json::Value, expected: &serde_json::Value) -> bool {
    match (actual, expected) {
        (serde_json::Value::Object(actual), serde_json::Value::Object(expected)) => {
            expected.iter().all(|(key, value)| {
                actual
                    .get(key)
                    .is_some_and(|actual| json_contains(actual, value))
            })
        }
        _ => actual == expected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tool_testing::{ScenarioData, SerialScenarioStep};

    fn text(value: &str) -> ScenarioData {
        ScenarioData {
            text: Some(value.to_owned()),
            hex: None,
        }
    }

    // 正向步骤（等一条 TX **出现**）的兜底窗口 —— 它是「到不了就该大声失败」的上界，
    // 不是「期望它这么久内到」。第 36/37 轮量的就是这类固定墙钟预算被当成通过条件的代价。
    //
    // 实测环境：16 核空机，`target/r36-load.py` 配方（churn×4 = 64 个每轮再分配 1 MiB 的忙等 hog，
    // 且跑**整包** libtest —— 16 线程并行，才是这条用例见红的那个环境）；把窗口临时抬到 8000ms
    // 让每一步都能命中，量「步骤开始 → 命中」的墙钟（churn×4 13 轮 + cpu×2 6 轮）：
    //   · 派发命令 → 看到该任务第一条 TX：最坏 **3270ms**（cpu×2 最坏 606ms，空载 1ms）；
    //   · 同一个任务已跑起来的下一拍（fixture 的 `task:sleep_ms(40)` 周期）：最坏 **1000ms+**
    //     （8000ms 窗口下测得 630ms，空载 40ms；抬到 1000ms 作独立一档时在 churn×4 复红 1/8）。
    // 两类**共用同一条派发链** —— 命令 → PluginManager 转发 → 插件线程被调度 → 写 →
    // 端口 worker 发布 → runner 排空订阅，每一环都要抢 CPU —— 所以不分别给数：
    // 第一版按 1.5 倍把「下一拍」单独压到 1000ms，自己就成了新的红点。
    // 取 5000ms = 实测最坏 3270ms 的 ≈1.5 倍。抬窗口**不花时间**：`expect_event` 每 1ms 轮询、
    // 命中即刻返回，空载这两类步骤实际仍是 1ms 与 40ms，整包 `cargo test -p tool-extension --lib`
    // 仍是 0.26s。反向的缺席检查不继承这个数 —— 它会把 80ms 的有界检查变成 5s 白等。
    const TX_BACKSTOP_MS: u64 = 5_000;

    #[test]
    fn scenario_covers_retry_cancel_and_timeout() {
        let root = std::env::temp_dir().join(format!(
            "hardware-workbench-scenario-{}",
            tool_core::now_timestamp_ms()
        ));
        let plugin = root.join("scenario.fixture");
        std::fs::create_dir_all(&plugin).unwrap();
        std::fs::write(
            plugin.join("plugin.json"),
            r#"{
          "id":"scenario.fixture","name":"Scenario Fixture","version":"0.1.0",
          "runtime":"lua","main":"main.lua","permissions":["serial","task"]
        }"#,
        )
        .unwrap();
        std::fs::write(
            plugin.join("main.lua"),
            r#"
local current = nil
local sequence = 0
ctx.commands.register("scenario.retry", function(payload)
  sequence = sequence + 1
  current = "scenario.retry." .. tostring(sequence)
  local port = payload.context.send.target_port
  ctx.task.start({ id = current, cancellable = true }, function(task)
    for _ = 1, 3 do
      if task:is_cancelled() then return end
      ctx.serial.write_line(port, "PING")
      task:sleep_ms(40)
    end
  end)
end)
ctx.commands.register("scenario.cancel", function()
  if current then ctx.task.cancel(current) end
end)
"#,
        )
        .unwrap();

        let scenario = SerialPluginScenario {
            name: "retry-cancel-timeout".to_owned(),
            plugin_id: "scenario.fixture".to_owned(),
            port: "TEST".to_owned(),
            // 正向 ExpectTx 一律继承这个兜底窗口；反向的缺席检查必须自己写窗口（见下）。
            timeout_ms: TX_BACKSTOP_MS,
            steps: vec![
                SerialScenarioStep::Execute {
                    command: "scenario.retry".to_owned(),
                    input: String::new(),
                    payload: serde_json::Value::Null,
                },
                SerialScenarioStep::ExpectTx {
                    data: text("PING\n"),
                    timeout_ms: None,
                },
                SerialScenarioStep::ExpectTx {
                    data: text("PING\n"),
                    timeout_ms: None,
                },
                SerialScenarioStep::ExpectTx {
                    data: text("PING\n"),
                    timeout_ms: None,
                },
                SerialScenarioStep::Execute {
                    command: "scenario.retry".to_owned(),
                    input: String::new(),
                    payload: serde_json::Value::Null,
                },
                SerialScenarioStep::ExpectTx {
                    data: text("PING\n"),
                    timeout_ms: None,
                },
                SerialScenarioStep::Cancel {
                    command: "scenario.cancel".to_owned(),
                },
                // 有界缺席检查：这 80ms 内不得出现任何 TX。两个方向的坑来自同一件事 ——
                // 取消命令的派发与 TX 事件的送达都要抢 CPU（实测派发最坏 3.3s，端口 worker 还会把
                // 相隔 40ms 的三次写攒进同一毫秒发布）：
                //   · 假红（实测抓到）：取消前已写出、还没被前面步骤消费的 PING 留在 backlog 里，
                //     这一步一进来 0ms 就命中 `等待窗口内出现了意外 TX` —— 改前改后同配方都见红；
                //   · 假绿（结构性，本次没抓到实例）：3 拍循环经常在取消送达前就跑完（19 轮探测里
                //     观测到 4 / 5 / 6 条 PING，空载恰好 4 条 = 第一次 3 条 + 第二次 1 条），
                //     窗口里静悄悄并不等于「循环被取消了」。
                // 抬高窗口买不到证明力 —— 要越过派发延迟（≈3.3s）才有声音，代价是每次通过都白等
                // 3.3s。所以维持 80ms，缺的那半条契约与关闭它的代价如实记在 docs/ARCHITECTURE.md。
                SerialScenarioStep::ExpectNoTx {
                    timeout_ms: Some(80),
                },
            ],
        };
        let report = SerialPluginScenarioRunner::new()
            .run(&root, &scenario)
            .unwrap();
        assert_eq!(
            report.cases[0].status,
            TestStatus::Passed,
            "{:?}",
            report.cases[0].error
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

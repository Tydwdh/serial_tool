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
                    let event =
                        self.expect_event(timeout_ms.unwrap_or(scenario.timeout_ms), |event| {
                            event.topic == serial_topics::SERIAL_TX
                                && event.meta_str("port") == Some(scenario.port.as_str())
                        });
                    if event.is_none() {
                        Ok(())
                    } else {
                        Err("等待窗口内出现了意外 TX".to_owned())
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
            .map(|event| TestPacketLog {
                id: event.id,
                timestamp_ms: event.timestamp_ms,
                topic: event.topic.clone(),
                direction: event.direction,
                payload_text: event.payload.text_lossy(),
                payload_hex: event
                    .payload
                    .as_bytes()
                    .unwrap_or_default()
                    .iter()
                    .map(|byte| format!("{byte:02X}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            })
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
                        "target_port": self.transport.open_ports().first().cloned().unwrap_or_default(),
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
            timeout_ms: 500,
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

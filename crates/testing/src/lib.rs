use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::Path;
use tool_core::{Direction, Payload, topics};
use tool_databus::{DataBus, Subscription, TopicFilter};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TestStatus {
    Passed,
    Failed,
    Skipped,
    Timeout,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TestPacketLog {
    pub id: u64,
    pub timestamp_ms: u64,
    pub topic: String,
    pub direction: Direction,
    pub payload_text: String,
    pub payload_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TestCaseResult {
    pub name: String,
    pub status: TestStatus,
    pub duration_ms: u64,
    #[serde(default)]
    pub logs: Vec<String>,
    #[serde(default)]
    pub assertions: u64,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub raw_packets: Vec<TestPacketLog>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TestRunReport {
    pub run_id: String,
    pub source: String,
    pub script_name: String,
    pub started_ms: u64,
    pub finished_ms: u64,
    #[serde(default)]
    pub cases: Vec<TestCaseResult>,
}

impl TestRunReport {
    pub fn passed_count(&self) -> usize {
        self.cases
            .iter()
            .filter(|case| case.status == TestStatus::Passed)
            .count()
    }

    pub fn failed_count(&self) -> usize {
        self.cases
            .iter()
            .filter(|case| matches!(case.status, TestStatus::Failed | TestStatus::Timeout))
            .count()
    }

    pub fn duration_ms(&self) -> u64 {
        self.finished_ms.saturating_sub(self.started_ms)
    }
}

pub struct TestReportStore {
    subscription: Subscription,
    reports: VecDeque<TestRunReport>,
    max_reports: usize,
    last_error: Option<String>,
}

/// 串口插件的端到端场景。JSON 文件可直接交给
/// `tool-extension` 的 `plugin_scenario` 示例运行。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SerialPluginScenario {
    pub name: String,
    pub plugin_id: String,
    #[serde(default = "default_virtual_port")]
    pub port: String,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    pub steps: Vec<SerialScenarioStep>,
}

fn default_virtual_port() -> String {
    "TEST".to_owned()
}
fn default_timeout_ms() -> u64 {
    1_000
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SerialScenarioStep {
    Rx {
        #[serde(flatten)]
        data: ScenarioData,
    },
    Execute {
        command: String,
        #[serde(default)]
        input: String,
        #[serde(default)]
        payload: serde_json::Value,
    },
    ExpectTx {
        #[serde(flatten)]
        data: ScenarioData,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    ExpectNoTx {
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    ExpectEvent {
        topic: String,
        #[serde(default)]
        payload: Option<serde_json::Value>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    Wait {
        ms: u64,
    },
    Cancel {
        command: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ScenarioData {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub hex: Option<String>,
}

impl ScenarioData {
    pub fn bytes(&self) -> Result<Vec<u8>, String> {
        match (&self.text, &self.hex) {
            (Some(text), None) => Ok(text.as_bytes().to_vec()),
            (None, Some(hex)) => {
                let compact: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
                if !compact.len().is_multiple_of(2) {
                    return Err("hex data must contain complete bytes".to_owned());
                }
                (0..compact.len())
                    .step_by(2)
                    .map(|index| {
                        u8::from_str_radix(&compact[index..index + 2], 16)
                            .map_err(|e| e.to_string())
                    })
                    .collect()
            }
            _ => Err("scenario data requires exactly one of text or hex".to_owned()),
        }
    }
}

impl TestReportStore {
    pub fn new(bus: &DataBus) -> Self {
        Self {
            subscription: bus.subscribe(TopicFilter::exact(topics::TEST_RESULT)),
            reports: VecDeque::new(),
            max_reports: 100,
            last_error: None,
        }
    }

    pub fn ingest(&mut self) {
        for event in self.subscription.drain() {
            let Payload::Json(value) = event.payload else {
                continue;
            };

            match serde_json::from_value::<TestRunReport>(value) {
                Ok(report) => self.upsert(report),
                Err(error) => self.last_error = Some(error.to_string()),
            }
        }
    }

    pub fn reports(&self) -> impl DoubleEndedIterator<Item = &TestRunReport> {
        self.reports.iter()
    }

    pub fn latest(&self) -> Option<&TestRunReport> {
        self.reports.back()
    }

    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn save_latest_json(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let Some(report) = self.latest() else {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no test report"));
        };
        if let Some(parent) = path.as_ref().parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(report)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        fs::write(path, text)
    }

    fn upsert(&mut self, report: TestRunReport) {
        if let Some(existing) = self
            .reports
            .iter_mut()
            .find(|existing| existing.run_id == report.run_id)
        {
            *existing = report;
        } else {
            self.reports.push_back(report);
            while self.reports.len() > self.max_reports {
                self.reports.pop_front();
            }
        }
    }
}

#[cfg(any(unix, windows))]
static UNOPENABLE_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// 一个「`metadata` 拿得到、当文件**打开必失败**」的临时路径，`Drop` 时自动清理。
///
/// 为什么需要它：第 43 轮那三条用例（`crates/recorder/src/replay.rs` 两条、
/// `crates/application/tests/headless.rs` 一条）拿**目录**当"打不开"的夹具，注释里写的
/// 「Linux 是目录 ⇒ `EISDIR`」是错的 —— POSIX 只在**写**打开时给 `EISDIR`，
/// `open(目录, O_RDONLY)` 是成功的，于是那句
/// `File::open(&dir).expect_err("目录不能当文件打开")` 在 Linux 上当场 panic：
/// 从这些用例进来的那一版起，Linux 作业（`cargo test --workspace --all-targets`）一直是红的，
/// 红在"测试前提"而不是被测代码上。
///
/// 各平台挑一个 stat 得到、打开必失败的形状：
/// * Windows：目录（`File::open` 给 `ERROR_ACCESS_DENIED` ⇒ `PermissionDenied`）；
/// * Unix：Unix 域套接字文件（`open(O_RDONLY)` 给 `ENXIO`，`File::create` 给 `ENXIO` 同样成立），
///   它**在 root 下也拦得住** —— 权限类夹具（`chmod 000`）跑在 root 上会被 `CAP_DAC_OVERRIDE`
///   整个绕过，本机与容器里的 root 复跑会得到"夹具其实能打开"的假绿。
#[cfg(any(unix, windows))]
pub struct UnopenablePath {
    path: std::path::PathBuf,
}

#[cfg(any(unix, windows))]
impl UnopenablePath {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "hw-unopenable-{}-{}",
            std::process::id(),
            UNOPENABLE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        #[cfg(windows)]
        fs::create_dir_all(&path).expect("创建「目录」形状的不可打开夹具失败");
        #[cfg(unix)]
        {
            // 上一次运行崩在中间会留下同名套接字文件，先清掉，否则 `bind` 给 `EADDRINUSE`。
            let _ = fs::remove_file(&path);
            std::os::unix::net::UnixListener::bind(&path)
                .expect("创建「Unix 域套接字」形状的不可打开夹具失败");
        }
        Self { path }
    }

    /// 夹具路径本体；调用方拿它去跑裸操作与被测出口，两边的路径必须逐字相同。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(any(unix, windows))]
impl Default for UnopenablePath {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(unix, windows))]
impl Drop for UnopenablePath {
    fn drop(&mut self) {
        // 两种形状各删一次，对不上的那一次是无害的空操作（失败也没别的办法）。
        if fs::remove_file(&self.path).is_err() {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tool_core::{Direction, Event};

    #[test]
    fn store_ingests_report_events() {
        let bus = DataBus::new();
        let mut store = TestReportStore::new(&bus);

        bus.publish(Event::new(
            topics::TEST_RESULT,
            "test",
            Direction::Internal,
            Payload::Json(json!({
                "run_id": "run-1",
                "source": "lua",
                "script_name": "test.lua",
                "started_ms": 1,
                "finished_ms": 2,
                "cases": [
                    {
                        "name": "case",
                        "status": "passed",
                        "duration_ms": 1,
                        "logs": [],
                        "assertions": 1,
                        "error": null,
                        "raw_packets": []
                    }
                ]
            })),
        ));
        store.ingest();

        let latest = store.latest().unwrap();
        assert_eq!(latest.run_id, "run-1");
        assert_eq!(latest.passed_count(), 1);
    }
}

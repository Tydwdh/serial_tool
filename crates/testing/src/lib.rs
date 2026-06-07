use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::Path;
use tool_core::{Payload, topics};
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
    pub direction: String,
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

pub type TestManager = TestReportStore;

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
            return;
        }

        self.reports.push_back(report);
        while self.reports.len() > self.max_reports {
            self.reports.pop_front();
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

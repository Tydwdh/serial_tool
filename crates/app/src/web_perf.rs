//! Browser-side performance diagnostics.
//!
//! The Native composition root already reports the important frame, transport,
//! DataBus and panel counters.  Keep the Web implementation deliberately
//! small and allocation-bounded, but expose the same signals through the
//! browser console so a real device run can be compared with Native.

use std::collections::VecDeque;

use tool_databus::DataBusPerfSnapshot;
use wasm_bindgen::JsValue;
use web_sys::console;

const SAMPLE_CAPACITY: usize = 2048;
const REPORT_INTERVAL_SECONDS: f64 = 5.0;

#[derive(Default)]
struct DurationSamples {
    values_ms: VecDeque<f64>,
}

impl DurationSamples {
    fn push_ms(&mut self, value: f64) {
        if self.values_ms.len() == SAMPLE_CAPACITY {
            self.values_ms.pop_front();
        }
        self.values_ms.push_back(value.max(0.0));
    }

    fn percentile(&self, percentile: f64) -> f64 {
        let mut values = self.values_ms.iter().copied().collect::<Vec<_>>();
        if values.is_empty() {
            return 0.0;
        }
        values.sort_by(f64::total_cmp);
        let index = ((values.len() - 1) as f64 * percentile.clamp(0.0, 1.0)).round() as usize;
        values[index]
    }

    fn latest(&self) -> f64 {
        self.values_ms.back().copied().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct WebRecorderPerf {
    pub(crate) running: bool,
    pub(crate) queued_events: u64,
    pub(crate) queued_bytes: u64,
    pub(crate) seconds_behind: f64,
    pub(crate) recorded_events: u64,
    pub(crate) recorded_bytes: u64,
    pub(crate) write_bytes_per_sec: u64,
    pub(crate) incomplete: bool,
}

fn now_seconds() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map(|performance| performance.now() / 1000.0)
        .unwrap_or(0.0)
}

/// Bounded Web performance telemetry.  The latest report is emitted at most
/// once every five seconds; this must never become another source of console
/// or allocation pressure during a high-rate serial session.
#[derive(Default)]
pub(crate) struct WebPerfDiagnostics {
    frame: DurationSamples,
    terminal_ingest: DurationSamples,
    terminal_render: DurationSamples,
    log_ingest: DurationSamples,
    log_render: DurationSamples,
    chart_render: DurationSamples,
    plugin_callback: DurationSamples,
    last_bus: Option<(f64, DataBusPerfSnapshot)>,
    last_report_at: Option<f64>,
    last_rx_bytes_per_sec: f64,
    last_tx_bytes_per_sec: f64,
    last_events_per_sec: f64,
    last_databus_publish_ms: f64,
    last_terminal_ingest_events: u64,
    last_log_ingest_events: u64,
}

impl WebPerfDiagnostics {
    pub(crate) fn begin_frame(&self) -> f64 {
        now_seconds()
    }

    pub(crate) fn record_terminal_render(&mut self, started_at: f64) {
        self.terminal_render
            .push_ms((now_seconds() - started_at) * 1000.0);
    }

    pub(crate) fn record_terminal_ingest(&mut self, started_at: f64, events: usize) {
        self.terminal_ingest
            .push_ms((now_seconds() - started_at) * 1000.0);
        self.last_terminal_ingest_events = events as u64;
    }

    pub(crate) fn record_log_render(&mut self, started_at: f64) {
        self.log_render
            .push_ms((now_seconds() - started_at) * 1000.0);
    }

    pub(crate) fn record_log_ingest(&mut self, started_at: f64, events: usize) {
        self.log_ingest
            .push_ms((now_seconds() - started_at) * 1000.0);
        self.last_log_ingest_events = events as u64;
    }

    pub(crate) fn record_chart_render(&mut self, started_at: f64) {
        self.chart_render
            .push_ms((now_seconds() - started_at) * 1000.0);
    }

    pub(crate) fn record_plugin_callback(&mut self, started_at: f64) {
        self.plugin_callback
            .push_ms((now_seconds() - started_at) * 1000.0);
    }

    pub(crate) fn end_frame(
        &mut self,
        started_at: f64,
        bus_snapshot: Option<DataBusPerfSnapshot>,
        recorder: WebRecorderPerf,
    ) {
        let now = now_seconds();
        self.frame.push_ms((now - started_at) * 1000.0);

        if let Some(snapshot) = bus_snapshot {
            if let Some((previous_at, previous)) = self.last_bus.replace((now, snapshot)) {
                let seconds = (now - previous_at).max(0.001);
                let event_delta = snapshot
                    .publish_count
                    .saturating_sub(previous.publish_count);
                let rx_delta = snapshot.rx_bytes.saturating_sub(previous.rx_bytes);
                let tx_delta = snapshot.tx_bytes.saturating_sub(previous.tx_bytes);
                self.last_events_per_sec = event_delta as f64 / seconds;
                self.last_rx_bytes_per_sec = rx_delta as f64 / seconds;
                self.last_tx_bytes_per_sec = tx_delta as f64 / seconds;
                if event_delta > 0 {
                    let publish_nanos = snapshot
                        .publish_nanos
                        .saturating_sub(previous.publish_nanos);
                    self.last_databus_publish_ms =
                        publish_nanos as f64 / event_delta as f64 / 1_000_000.0;
                }
            } else {
                self.last_bus = Some((now, snapshot));
            }

            if self
                .last_report_at
                .is_none_or(|last| now - last >= REPORT_INTERVAL_SECONDS)
            {
                self.last_report_at = Some(now);
                let report = format!(
                    "perf frame p50/p95/p99={:.2}/{:.2}/{:.2}ms rx={:.0}B/s tx={:.0}B/s events={:.0}/s databus={:.3}ms subscriber={}/{}B drop={} recorder={} queued={}/{}B behind={:.1}s events={} bytes={} write={}/s incomplete={} ingest(term/log)={}/{:.2}/{}/{:.2}ms render(term/log/chart)={:.2}/{:.2}/{:.2}ms plugin={:.2}ms",
                    self.frame.percentile(0.50),
                    self.frame.percentile(0.95),
                    self.frame.percentile(0.99),
                    self.last_rx_bytes_per_sec,
                    self.last_tx_bytes_per_sec,
                    self.last_events_per_sec,
                    self.last_databus_publish_ms,
                    snapshot.subscriber_queued_events,
                    snapshot.subscriber_queued_bytes,
                    snapshot.subscriber_dropped,
                    recorder.running,
                    recorder.queued_events,
                    recorder.queued_bytes,
                    recorder.seconds_behind,
                    recorder.recorded_events,
                    recorder.recorded_bytes,
                    recorder.write_bytes_per_sec,
                    recorder.incomplete,
                    self.last_terminal_ingest_events,
                    self.terminal_ingest.latest(),
                    self.last_log_ingest_events,
                    self.log_ingest.latest(),
                    self.terminal_render.latest(),
                    self.log_render.latest(),
                    self.chart_render.latest(),
                    self.plugin_callback.latest(),
                );
                console::info_1(&JsValue::from_str(&report));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DurationSamples;

    #[test]
    fn duration_samples_are_bounded_and_ordered() {
        let mut samples = DurationSamples::default();
        samples.push_ms(4.0);
        samples.push_ms(1.0);
        samples.push_ms(3.0);
        assert_eq!(samples.percentile(0.50), 3.0);
        assert_eq!(samples.percentile(0.99), 4.0);
    }
}

use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tool_application::perf::ApplicationPerfSnapshot;

const SAMPLE_CAPACITY: usize = 2048;

#[derive(Debug, Clone, Default)]
pub(crate) struct PerfReport {
    pub frame_p50_ms: f64,
    pub frame_p95_ms: f64,
    pub frame_p99_ms: f64,
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
    pub events_per_sec: f64,
    pub terminal_ingest_events: u64,
    pub terminal_ingest_ms: f64,
    pub terminal_render_ms: f64,
    pub log_ingest_events: u64,
    pub log_ingest_ms: f64,
    pub log_render_ms: f64,
    pub databus_publish_ms: f64,
    pub subscriber_backlog_events: u64,
    pub subscriber_backlog_bytes: u64,
    pub subscriber_dropped: u64,
    pub recorder_backlog_events: u64,
    pub recorder_backlog_bytes: u64,
    pub recorder_seconds_behind: f64,
    pub recorder_write_bytes_per_sec: u64,
    pub plugin_callback_ms: f64,
    pub chart_render_ms: f64,
}

#[derive(Default)]
struct DurationSamples {
    values_ms: VecDeque<f64>,
}

impl DurationSamples {
    fn push(&mut self, duration: Duration) {
        if self.values_ms.len() == SAMPLE_CAPACITY {
            self.values_ms.pop_front();
        }
        self.values_ms.push_back(duration.as_secs_f64() * 1000.0);
    }

    fn percentile(&self, percentile: f64) -> f64 {
        percentile_of(self.values_ms.iter().copied(), percentile)
    }

    fn latest(&self) -> f64 {
        self.values_ms.back().copied().unwrap_or_default()
    }
}

fn percentile_of(values: impl IntoIterator<Item = f64>, percentile: f64) -> f64 {
    let mut values: Vec<f64> = values.into_iter().collect();
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f64::total_cmp);
    let index = ((values.len() - 1) as f64 * percentile.clamp(0.0, 1.0)).round() as usize;
    values[index]
}

#[derive(Default)]
pub(crate) struct PerfDiagnostics {
    frame: DurationSamples,
    terminal_ingest: DurationSamples,
    terminal_render: DurationSamples,
    log_ingest: DurationSamples,
    log_render: DurationSamples,
    databus_publish: DurationSamples,
    plugin_callback: DurationSamples,
    chart_render: DurationSamples,
    last_terminal_ingest_events: u64,
    last_log_ingest_events: u64,
    last_bus: Option<(Instant, tool_databus::DataBusPerfSnapshot)>,
    last_report: Option<PerfReport>,
    last_log_at: Option<Instant>,
}

impl PerfDiagnostics {
    pub(crate) fn begin_frame(&self) -> Instant {
        Instant::now()
    }

    pub(crate) fn end_frame(&mut self, started: Instant, snapshot: ApplicationPerfSnapshot) {
        self.frame.push(started.elapsed());
        self.observe_application(snapshot);
    }

    pub(crate) fn record_terminal_ingest(&mut self, started: Instant, events: usize) {
        self.terminal_ingest.push(started.elapsed());
        self.last_terminal_ingest_events = events as u64;
    }

    pub(crate) fn record_log_ingest(&mut self, started: Instant, events: usize) {
        self.log_ingest.push(started.elapsed());
        self.last_log_ingest_events = events as u64;
    }

    pub(crate) fn record_terminal_render(&mut self, started: Instant) {
        self.terminal_render.push(started.elapsed());
    }

    pub(crate) fn record_log_render(&mut self, started: Instant) {
        self.log_render.push(started.elapsed());
    }

    pub(crate) fn record_plugin_callback(&mut self, started: Instant) {
        self.plugin_callback.push(started.elapsed());
    }

    pub(crate) fn record_chart_render(&mut self, started: Instant) {
        self.chart_render.push(started.elapsed());
    }

    fn observe_application(&mut self, snapshot: ApplicationPerfSnapshot) {
        let now = Instant::now();
        let mut rx_bytes_per_sec = 0.0;
        let mut tx_bytes_per_sec = 0.0;
        let mut events_per_sec = 0.0;
        if let Some((previous_at, previous)) = self.last_bus.take() {
            let seconds = now.duration_since(previous_at).as_secs_f64().max(0.001);
            let event_delta = snapshot
                .databus
                .publish_count
                .saturating_sub(previous.publish_count);
            let rx_delta = snapshot.databus.rx_bytes.saturating_sub(previous.rx_bytes);
            let tx_delta = snapshot.databus.tx_bytes.saturating_sub(previous.tx_bytes);
            events_per_sec = event_delta as f64 / seconds;
            rx_bytes_per_sec = rx_delta as f64 / seconds;
            tx_bytes_per_sec = tx_delta as f64 / seconds;
            if event_delta > 0 {
                let nanos = snapshot
                    .databus
                    .publish_nanos
                    .saturating_sub(previous.publish_nanos);
                if let Some(average_nanos) = nanos.checked_div(event_delta) {
                    self.databus_publish
                        .push(Duration::from_nanos(average_nanos));
                }
            }
        }
        self.last_bus = Some((now, snapshot.databus));

        let recorder = snapshot.recorder;
        let report = PerfReport {
            frame_p50_ms: self.frame.percentile(0.50),
            frame_p95_ms: self.frame.percentile(0.95),
            frame_p99_ms: self.frame.percentile(0.99),
            rx_bytes_per_sec,
            tx_bytes_per_sec,
            events_per_sec,
            terminal_ingest_events: self.last_terminal_ingest_events,
            terminal_ingest_ms: self.terminal_ingest.latest(),
            terminal_render_ms: self.terminal_render.latest(),
            log_ingest_ms: self.log_ingest.latest(),
            log_ingest_events: self.last_log_ingest_events,
            log_render_ms: self.log_render.latest(),
            databus_publish_ms: self.databus_publish.latest(),
            subscriber_backlog_events: snapshot.databus.subscriber_queued_events,
            subscriber_backlog_bytes: snapshot.databus.subscriber_queued_bytes,
            subscriber_dropped: snapshot.databus.subscriber_dropped,
            recorder_backlog_events: recorder.queued_events,
            recorder_backlog_bytes: recorder.queued_bytes,
            recorder_seconds_behind: recorder.seconds_behind,
            recorder_write_bytes_per_sec: recorder.write_throughput_bytes_per_sec,
            plugin_callback_ms: self.plugin_callback.latest(),
            chart_render_ms: self.chart_render.latest(),
        };
        self.last_report = Some(report);

        if self
            .last_log_at
            .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(5))
        {
            self.last_log_at = Some(now);
            let report = self.last_report.as_ref().expect("report just stored");
            log::info!(
                "perf frame p50/p95/p99={:.2}/{:.2}/{:.2}ms rx={:.0}B/s tx={:.0}B/s events={:.0}/s databus={:.3}ms term_ingest={}/{:.2}ms term_render={:.2}ms log_ingest={}/{:.2}ms log_render={:.2}ms subscriber={}/{}B drop={} recorder={}/{}B/{:.1}s write={}/s plugin={:.2}ms chart={:.2}ms",
                report.frame_p50_ms,
                report.frame_p95_ms,
                report.frame_p99_ms,
                report.rx_bytes_per_sec,
                report.tx_bytes_per_sec,
                report.events_per_sec,
                report.databus_publish_ms,
                report.terminal_ingest_events,
                report.terminal_ingest_ms,
                report.terminal_render_ms,
                report.log_ingest_events,
                report.log_ingest_ms,
                report.log_render_ms,
                report.subscriber_backlog_events,
                report.subscriber_backlog_bytes,
                report.subscriber_dropped,
                report.recorder_backlog_events,
                report.recorder_backlog_bytes,
                report.recorder_seconds_behind,
                report.recorder_write_bytes_per_sec,
                report.plugin_callback_ms,
                report.chart_render_ms,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::percentile_of;

    #[test]
    fn percentile_is_stable_for_small_samples() {
        assert_eq!(percentile_of([1.0, 2.0, 3.0, 4.0], 0.50), 3.0);
        assert_eq!(percentile_of([1.0, 2.0, 3.0, 4.0], 0.99), 4.0);
    }
}

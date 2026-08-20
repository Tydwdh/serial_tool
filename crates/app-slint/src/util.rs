pub fn fmt_ts(ms: u64) -> String {
    let Some(dt_utc) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms as i64) else {
        return "--:--:--.---".to_owned();
    };
    dt_utc
        .with_timezone(&chrono::Local)
        .format("%H:%M:%S%.3f")
        .to_string()
}

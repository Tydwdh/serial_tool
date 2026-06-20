//! Lua 全局表键名与 `yield_op` 字段名常量。
//!
//! `event_loop`、`api::serial`、`api::task` 之间通过 Lua 全局表
//! (`__plugin_tasks` / `__plugin_timers` / `__plugin_callbacks` / `__current_task_id`)
//! 和 task state 的 `yield_op` 字段隐式契约协作。把这些字符串字面量集中为常量，
//! 避免拆分模块时因拼写不一致埋下 bug，并提供单一事实源。

// ── Lua 全局表键 ──
pub const PLUGIN_CALLBACKS: &str = "__plugin_callbacks";
pub const PLUGIN_TIMERS: &str = "__plugin_timers";
pub const PLUGIN_TASKS: &str = "__plugin_tasks";
pub const PLUGIN_STORAGE: &str = "__plugin_storage";
pub const PLUGIN_DISABLE: &str = "__plugin_disable";
pub const CURRENT_TASK_ID: &str = "__current_task_id";

// ── task state 字段名 ──
pub const TASK_YIELD_OP: &str = "yield_op";
pub const TASK_FINISHED: &str = "finished";
pub const TASK_CANCELLED: &str = "cancelled";

// ── yield_op 字段名 ──
pub const YIELD_KIND: &str = "kind";
pub const YIELD_PORT: &str = "port";
pub const YIELD_TIMEOUT_MS: &str = "timeout_ms";
pub const YIELD_DEADLINE_MS: &str = "deadline_ms";

// ── yield_op.kind 枚举值（生产端 api::serial/api::task 与消费端 process_tasks 共用） ──
pub const YIELD_READ_LINE: &str = "read_line";
pub const YIELD_WRITE_LINE_AND_EXPECT: &str = "write_line_and_expect";
pub const YIELD_SLEEP: &str = "sleep";
pub const YIELD_WAIT_PAUSED: &str = "wait_paused";

// ── write_line_and_expect 的 pattern entry 字段名 ──
pub const EXPECT_PATTERN: &str = "pattern";
pub const EXPECT_ACTION: &str = "action";
pub const EXPECT_ACTION_RETURN: &str = "return";

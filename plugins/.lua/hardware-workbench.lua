---@meta hardware-workbench

---@alias HwJsonValue nil|boolean|number|string|table
---@alias HwLogLevel "trace"|"debug"|"info"|"warn"|"error"

---@class HwPluginInfo
---@field id string
---@field name string
---@field version string
---@field api_version string
---@field runtime string
---@field root string
---@field permissions string[]
---@field contributes table

---@class HwEvent
---@field id integer
---@field timestamp_ms integer
---@field topic string
---@field source string
---@field direction string
---@field payload any
---@field metadata table<string, any>

---@class HwLogApi: table<string, fun(message: string)>
---@field trace fun(message: string)
---@field debug fun(message: string)
---@field info fun(message: string)
---@field warn fun(message: string)
---@field error fun(message: string)

---@class HwBusApi
---@field publish fun(topic: string, payload?: HwJsonValue)
---@field history fun(topic_prefix?: string): HwEvent[]
---@field wait fun(topic: string, timeout_ms?: integer): HwEvent?
---@field subscribe fun(topic_prefix: string, timeout_ms?: integer): HwEvent?
---@field on fun(topic: string, callback: fun(event: HwEvent))
---@field off fun(topic: string)
---
--- 串口生命周期事件（通过 ctx.bus.on 监听）：
---   transport.serial.opened  — payload: { port: string, baud_rate: integer }
---   transport.serial.closed  — payload: { port: string, baud_rate: integer }

---@class HwSerialPortInfo
---@field port_name string
---@field port_type string

---@class HwSerialStatus
---@field open boolean
---@field port_name string?
---@field baud_rate integer?

---@class HwSerialOpenConfig
---@field port_name string
---@field baud_rate integer?
---@field data_bits integer?
---@field parity string?
---@field stop_bits integer?
---@field timeout_ms integer?
---@field dtr boolean?
---@field rts boolean?

---@class HwSerialOpenEvent
---@field port string
---@field baud_rate integer

---@class HwSerialCloseEvent
---@field port string
---@field baud_rate integer

---@class HwSerialRequestOptions
---@field port string
---@field tx string
---@field expect string
---@field timeout_ms integer?

---@class HwSerialExpectPattern
---@field name string?
---@field pattern string
---@field action "return"|"continue"?

---@class HwSerialWriteExpectOptions
---@field timeout_ms integer?
---@field delimiter string?
---@field flush_before_send boolean?
---@field patterns HwSerialExpectPattern[]?

---@class HwSerialReadLineResult
---@field line string?
---@field err string?

---@class HwSerialMatchedResult
---@field name string?
---@field line string?
---@field elapsed_ms integer?

---@class HwSerialWriteExpectResult
---@field result HwSerialMatchedResult?
---@field err string?

---@class HwSerialApi
---@field list fun(): HwSerialPortInfo[]
---@field open fun(config: HwSerialOpenConfig)
---@field close fun()
---@field close_port fun(port: string)
---@field send_to fun(port: string, text: string)
---@field send_hex_to fun(port: string, text: string)
---@field status_port fun(port: string): HwSerialStatus
---@field open_ports fun(): string[]
---@field expect fun(pattern: string, timeout_ms?: integer): string?
---@field expect_from fun(port: string, pattern: string, timeout_ms?: integer): string?
---@field request fun(options: HwSerialRequestOptions): string?
---@field read_line fun(port: string, options?: { timeout_ms?: integer }): HwSerialReadLineResult
---@field write_line_and_expect fun(port: string, line: string, options?: HwSerialWriteExpectOptions): HwSerialWriteExpectResult
---@field flush_rx fun(port: string)

---@class HwPanelConfig
---@field id string
---@field title string?
---@field [string] any

---@class HwLogEntry
---@field level HwLogLevel?
---@field message string

---@class HwUiApi
---@field create_chart fun(config: HwPanelConfig)
---@field create_form fun(config: HwPanelConfig)
---@field create_attitude fun(config: HwPanelConfig)
---@field create_gauge fun(config: HwPanelConfig)
---@field remove_panel fun(panel_id: string)
---@field get_panel fun(panel_id: string): table?
---@field set_value fun(panel_id: string, field_id: string, value: any)
---@field set_contribution_value fun(contribution_id: string, value: any)
---@field set_enabled fun(panel_id: string, field_id: string, enabled: boolean)
---@field set_visible fun(panel_id: string, field_id: string, visible: boolean)

---@class HwTimerApi
---@field after fun(ms: integer, callback: fun()): string
---@field every fun(ms: integer, callback: fun()): string
---@field cancel fun(id: string)

---@class HwStorageApi
---@field get fun(key: string): string?
---@field set fun(key: string, value: string)
---@field keys fun(): string[]

---@class HwDialogFilter
---@field name string
---@field extensions string[]

---@class HwDialogOpenFileConfig
---@field title string?
---@field filters HwDialogFilter[]?

---@class HwDialogApi
---@field open_file fun(config?: HwDialogOpenFileConfig): string?

---@class HwFsApi
---@field read_text fun(path: string): string
---@field read_lines fun(path: string): fun(): string?
---@field read_lines_stream fun(path: string): fun(): string?

---@class HwConfigApi
---@field get fun(key: string, default?: any): any
---@field set fun(key: string, value: any)
---@field remove fun(key: string)
---@field keys fun(): string[]
---@field profile_list fun(): string[]
---@field profile_load fun(name: string): table?
---@field profile_save fun(name: string, data: table)
---@field profile_delete fun(name: string)

---@class HwCommandPayload
---@field plugin_id string
---@field command string
---@field contribution_id string?
---@field slot string?
---@field kind string?
---@field args any
---@field context table?
---@field origin string?

---@class HwCommandsApi
---@field register fun(command: string, handler: fun(payload: HwCommandPayload))
---@field unregister fun(command: string)
---@field list fun(): string[]
---@field execute fun(command: string, args?: any)

---@class HwTaskConfig
---@field id string
---@field title string?
---@field cancellable boolean?
---@field pausable boolean?

---@class HwTask
---@field id string
---@field title string
---@field cancelled boolean
---@field paused boolean
---@field finished boolean
---@field progress_current integer
---@field progress_total integer
---@field progress_percent number?
---@field status string
---@field error string?
---@field is_cancelled fun(self: HwTask): boolean
---@field is_paused fun(self: HwTask): boolean
---@field sleep_ms fun(self: HwTask, ms: integer)
---@field wait_if_paused fun(self: HwTask)
---@field set_progress fun(self: HwTask, current: integer, total: integer)
---@field set_progress_percent fun(self: HwTask, percent: number)
---@field set_status fun(self: HwTask, text: string)
---@field log fun(self: HwTask, level: HwLogLevel, message: string)

---@class HwTaskApi
---@field start fun(config: HwTaskConfig, callback: fun(task: HwTask)): HwTask
---@field cancel fun(id: string)
---@field pause fun(id: string)
---@field resume fun(id: string)
---@field list fun(): HwTask[]

---@class HwReplayApi
---@field emit fun(topic: string, payload?: HwJsonValue)
---@field log fun(message: string)
---@field current_event fun(): HwEvent?

---@class HwTestApi
---@field [string] any

---@class HwContext
---@field plugin HwPluginInfo
---@field now_ms fun(): integer
---@field commands HwCommandsApi
---@field log HwLogApi
---@field bus HwBusApi
---@field serial HwSerialApi
---@field ui HwUiApi
---@field timer HwTimerApi
---@field session HwStorageApi
---@field dialog HwDialogApi
---@field fs HwFsApi
---@field config HwConfigApi
---@field task HwTaskApi
---@field replay HwReplayApi
---@field test HwTestApi

---@type HwContext
ctx = ctx

---@param callback fun()
function on_disable(callback) end

---@class HwReplaySession
---@field start_ms integer
---@field end_ms integer
---@field event_count integer

---@param session HwReplaySession
function on_replay_begin(session) end

---@param event HwEvent
function on_replay_event(event) end

function on_replay_end() end

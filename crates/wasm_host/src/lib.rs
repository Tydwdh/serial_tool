use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tool_core::{Event, LogLevel, Payload, topics};
use tool_databus::{DataBus, Subscription, TopicFilter};
use wasmtime::error::Context;
use wasmtime::{
    AsContext, AsContextMut, Caller, Engine, Instance, Linker, Memory, Module,
    Result as WasmtimeResult, Store, bail,
};

#[derive(Debug, Error)]
pub enum WasmHostError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("wat parse error: {0}")]
    Wat(#[from] wat::Error),
    #[error("wasm runtime error: {0}")]
    Wasmtime(#[from] wasmtime::Error),
    #[error("decoder '{0}' was not found")]
    NotFound(String),
    #[error("unsupported wasm decoder runtime '{0}'")]
    UnsupportedRuntime(String),
    #[error("decoder '{0}' does not export memory")]
    MissingMemory(String),
    #[error("decoder '{decoder_id}' input needs {required} byte(s), memory has {available}")]
    InputOutOfBounds {
        decoder_id: String,
        required: usize,
        available: usize,
    },
    #[error(
        "decoder '{decoder_id}' output buffer needs {required} byte(s), memory has {available}"
    )]
    OutputOutOfBounds {
        decoder_id: String,
        required: usize,
        available: usize,
    },
    #[error("decoder '{decoder_id}' returned {len} byte(s), output capacity is {capacity}")]
    OutputTooLarge {
        decoder_id: String,
        len: usize,
        capacity: usize,
    },
    #[error("decoder '{decoder_id}' rejected frame with code {code}")]
    DecodeRejected { decoder_id: String, code: i32 },
    #[error("wasm plugin '{plugin_id}' function '{function}' returned code {code}")]
    PluginRejected {
        plugin_id: String,
        function: String,
        code: i32,
    },
    #[error("decoder output was not utf-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
}

pub type WasmHostResult<T> = Result<T, WasmHostError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WasmPluginConfig {
    pub id: String,
    pub name: String,
    pub module_path: PathBuf,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub initial_subscriptions: Vec<String>,
    #[serde(default = "default_input_ptr")]
    pub input_ptr: u32,
    #[serde(default = "default_plugin_input_cap")]
    pub input_cap: u32,
}

impl WasmPluginConfig {
    pub fn new(id: impl Into<String>, name: impl Into<String>, module_path: PathBuf) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            module_path,
            permissions: Vec::new(),
            initial_subscriptions: Vec::new(),
            input_ptr: default_input_ptr(),
            input_cap: default_plugin_input_cap(),
        }
    }
}

pub struct WasmPluginRuntime {
    config: WasmPluginConfig,
    engine: Engine,
    module: Module,
    bus: DataBus,
    storage: Arc<Mutex<BTreeMap<String, String>>>,
    subscriptions: Arc<Mutex<BTreeSet<String>>>,
}

impl WasmPluginRuntime {
    pub fn load(bus: DataBus, config: WasmPluginConfig) -> WasmHostResult<Self> {
        let engine = Engine::default();
        let bytes = load_module_bytes(&config.module_path)?;
        let module = Module::new(&engine, bytes)?;
        let subscriptions = config.initial_subscriptions.iter().cloned().collect();

        Ok(Self {
            config,
            engine,
            module,
            bus,
            storage: Arc::new(Mutex::new(BTreeMap::new())),
            subscriptions: Arc::new(Mutex::new(subscriptions)),
        })
    }

    pub fn config(&self) -> &WasmPluginConfig {
        &self.config
    }

    pub fn activate(&self) -> WasmHostResult<bool> {
        self.call_no_args("activate")
    }

    pub fn deactivate(&self) -> WasmHostResult<bool> {
        self.call_no_args("deactivate")
    }

    pub fn on_event(&self, event: &Event) -> WasmHostResult<bool> {
        let input = serde_json::to_vec(event)?;
        self.call_with_input("on_event", &input)
    }

    pub fn is_subscribed(&self, topic: &str) -> bool {
        self.subscriptions
            .lock()
            .iter()
            .any(|filter| topic_matches(filter, topic))
    }

    pub fn subscriptions(&self) -> Vec<String> {
        self.subscriptions.lock().iter().cloned().collect()
    }

    fn call_no_args(&self, function: &str) -> WasmHostResult<bool> {
        let (mut store, instance) = self.instantiate()?;
        let Some(func) = instance.get_func(&mut store, function) else {
            return Ok(false);
        };
        let func = func.typed::<(), i32>(&store)?;
        let code = func.call(&mut store, ())?;
        self.check_plugin_code(function, code)?;
        Ok(true)
    }

    fn call_with_input(&self, function: &str, input: &[u8]) -> WasmHostResult<bool> {
        let (mut store, instance) = self.instantiate()?;
        let Some(func) = instance.get_func(&mut store, function) else {
            return Ok(false);
        };
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| WasmHostError::MissingMemory(self.config.id.clone()))?;
        let input_ptr = self.config.input_ptr as usize;
        let input_cap = self.config.input_cap as usize;
        if input.len() > input_cap {
            return Err(WasmHostError::InputOutOfBounds {
                decoder_id: self.config.id.clone(),
                required: input.len(),
                available: input_cap,
            });
        }
        let input_end =
            input_ptr
                .checked_add(input.len())
                .ok_or_else(|| WasmHostError::InputOutOfBounds {
                    decoder_id: self.config.id.clone(),
                    required: usize::MAX,
                    available: memory.data_size(&store),
                })?;
        let available = memory.data_size(&store);
        if input_end > available {
            return Err(WasmHostError::InputOutOfBounds {
                decoder_id: self.config.id.clone(),
                required: input_end,
                available,
            });
        }

        memory.data_mut(&mut store)[input_ptr..input_end].copy_from_slice(input);
        let func = func.typed::<(i32, i32), i32>(&store)?;
        let code = func.call(
            &mut store,
            (self.config.input_ptr as i32, input.len() as i32),
        )?;
        self.check_plugin_code(function, code)?;
        Ok(true)
    }

    fn instantiate(&self) -> WasmHostResult<(Store<PluginHostState>, Instance)> {
        let mut linker = Linker::new(&self.engine);
        register_plugin_host_functions(&mut linker)?;
        let state = PluginHostState {
            plugin_id: self.config.id.clone(),
            bus: self.bus.clone(),
            permissions: self.config.permissions.iter().cloned().collect(),
            storage: self.storage.clone(),
            subscriptions: self.subscriptions.clone(),
        };
        let mut store = Store::new(&self.engine, state);
        let instance = linker.instantiate(&mut store, &self.module)?;
        Ok((store, instance))
    }

    fn check_plugin_code(&self, function: &str, code: i32) -> WasmHostResult<()> {
        if code < 0 {
            return Err(WasmHostError::PluginRejected {
                plugin_id: self.config.id.clone(),
                function: function.to_owned(),
                code,
            });
        }
        Ok(())
    }
}

struct PluginHostState {
    plugin_id: String,
    bus: DataBus,
    permissions: BTreeSet<String>,
    storage: Arc<Mutex<BTreeMap<String, String>>>,
    subscriptions: Arc<Mutex<BTreeSet<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WasmDecoderManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default = "default_runtime")]
    pub runtime: String,
    pub module: String,
    #[serde(default = "default_function")]
    pub function: String,
    #[serde(default = "default_input_topic")]
    pub input_topic: String,
    #[serde(default = "default_output_topic")]
    pub output_topic: String,
    #[serde(default = "default_input_ptr")]
    pub input_ptr: u32,
    #[serde(default = "default_output_ptr")]
    pub output_ptr: u32,
    #[serde(default = "default_output_cap")]
    pub output_cap: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WasmDecoderState {
    Discovered,
    Enabled,
    Failed,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmDecoderSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub runtime: String,
    pub input_topic: String,
    pub output_topic: String,
    pub module: String,
    pub path: PathBuf,
    pub state: WasmDecoderState,
    pub decoded_count: u64,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub struct WasmDecoder {
    manifest: WasmDecoderManifest,
    engine: Engine,
    module: Module,
    module_path: PathBuf,
}

impl WasmDecoder {
    pub fn load(
        root: &Path,
        manifest: &WasmDecoderManifest,
        engine: &Engine,
    ) -> WasmHostResult<Self> {
        if manifest.runtime != "wasm" && manifest.runtime != "wasmtime" {
            return Err(WasmHostError::UnsupportedRuntime(manifest.runtime.clone()));
        }

        let module_path = root.join(&manifest.module);
        let bytes = load_module_bytes(&module_path)?;
        let module = Module::new(engine, bytes)?;

        Ok(Self {
            manifest: manifest.clone(),
            engine: engine.clone(),
            module,
            module_path,
        })
    }

    pub fn manifest(&self) -> &WasmDecoderManifest {
        &self.manifest
    }

    pub fn module_path(&self) -> &Path {
        &self.module_path
    }

    pub fn decode_bytes(&self, input: &[u8]) -> WasmHostResult<Option<Value>> {
        let mut store = Store::new(&self.engine, ());
        let instance = Instance::new(&mut store, &self.module, &[])?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| WasmHostError::MissingMemory(self.manifest.id.clone()))?;
        let decode = instance
            .get_typed_func::<(i32, i32, i32, i32), i32>(&mut store, &self.manifest.function)?;

        let input_ptr = self.manifest.input_ptr as usize;
        let output_ptr = self.manifest.output_ptr as usize;
        let output_cap = self.manifest.output_cap as usize;
        let input_end =
            input_ptr
                .checked_add(input.len())
                .ok_or_else(|| WasmHostError::InputOutOfBounds {
                    decoder_id: self.manifest.id.clone(),
                    required: usize::MAX,
                    available: memory.data_size(&store),
                })?;
        let output_end =
            output_ptr
                .checked_add(output_cap)
                .ok_or_else(|| WasmHostError::OutputOutOfBounds {
                    decoder_id: self.manifest.id.clone(),
                    required: usize::MAX,
                    available: memory.data_size(&store),
                })?;
        let available = memory.data_size(&store);
        if input_end > available {
            return Err(WasmHostError::InputOutOfBounds {
                decoder_id: self.manifest.id.clone(),
                required: input_end,
                available,
            });
        }
        if output_end > available {
            return Err(WasmHostError::OutputOutOfBounds {
                decoder_id: self.manifest.id.clone(),
                required: output_end,
                available,
            });
        }

        memory.data_mut(&mut store)[input_ptr..input_end].copy_from_slice(input);
        let output_len = decode.call(
            &mut store,
            (
                self.manifest.input_ptr as i32,
                input.len() as i32,
                self.manifest.output_ptr as i32,
                self.manifest.output_cap as i32,
            ),
        )?;

        if output_len == 0 {
            return Ok(None);
        }
        if output_len < 0 {
            return Err(WasmHostError::DecodeRejected {
                decoder_id: self.manifest.id.clone(),
                code: output_len,
            });
        }

        let output_len = output_len as usize;
        if output_len > output_cap {
            return Err(WasmHostError::OutputTooLarge {
                decoder_id: self.manifest.id.clone(),
                len: output_len,
                capacity: output_cap,
            });
        }

        let output = memory.data(&store)[output_ptr..output_ptr + output_len].to_vec();
        let text = String::from_utf8(output)?;
        Ok(Some(serde_json::from_str(&text)?))
    }
}

struct WasmDecoderRecord {
    manifest: WasmDecoderManifest,
    root: PathBuf,
    decoder: Option<WasmDecoder>,
    state: WasmDecoderState,
    decoded_count: u64,
    last_error: Option<String>,
}

pub struct WasmDecoderManager {
    bus: DataBus,
    engine: Engine,
    records: BTreeMap<String, WasmDecoderRecord>,
    roots: Vec<PathBuf>,
    subscription: Subscription,
}

impl WasmDecoderManager {
    pub fn new(bus: DataBus) -> Self {
        let subscription = bus.subscribe(TopicFilter::All);
        Self {
            bus,
            engine: Engine::default(),
            records: BTreeMap::new(),
            roots: Vec::new(),
            subscription,
        }
    }

    pub fn discover_roots(
        &mut self,
        roots: impl IntoIterator<Item = PathBuf>,
    ) -> WasmHostResult<usize> {
        self.roots = roots.into_iter().collect();
        self.refresh()
    }

    pub fn refresh(&mut self) -> WasmHostResult<usize> {
        let roots = self.roots.clone();
        let mut count = 0;
        for root in roots {
            count += self.discover_root(&root)?;
        }
        self.bus.publish(Event::system_log(
            LogLevel::Info,
            "wasm",
            format!("discovered {count} wasm decoder(s)"),
        ));
        Ok(count)
    }

    pub fn discover_root(&mut self, root: &Path) -> WasmHostResult<usize> {
        if !root.exists() {
            return Ok(0);
        }

        if root.join("decoder.json").exists() {
            self.discover_decoder_dir(root)?;
            return Ok(1);
        }

        let mut count = 0;
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() || !path.join("decoder.json").exists() {
                continue;
            }
            self.discover_decoder_dir(&path)?;
            count += 1;
        }
        Ok(count)
    }

    pub fn enable(&mut self, decoder_id: &str) -> WasmHostResult<()> {
        let record = self
            .records
            .get_mut(decoder_id)
            .ok_or_else(|| WasmHostError::NotFound(decoder_id.to_owned()))?;

        match WasmDecoder::load(&record.root, &record.manifest, &self.engine) {
            Ok(decoder) => {
                record.decoder = Some(decoder);
                record.state = WasmDecoderState::Enabled;
                record.last_error = None;
                self.bus.publish(Event::system_log(
                    LogLevel::Info,
                    format!("wasm:{decoder_id}"),
                    "decoder enabled",
                ));
                Ok(())
            }
            Err(error) => {
                record.state = WasmDecoderState::Failed;
                record.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    pub fn disable(&mut self, decoder_id: &str) -> WasmHostResult<()> {
        let record = self
            .records
            .get_mut(decoder_id)
            .ok_or_else(|| WasmHostError::NotFound(decoder_id.to_owned()))?;
        record.decoder = None;
        record.state = WasmDecoderState::Disabled;
        self.bus.publish(Event::system_log(
            LogLevel::Info,
            format!("wasm:{decoder_id}"),
            "decoder disabled",
        ));
        Ok(())
    }

    pub fn summaries(&self) -> Vec<WasmDecoderSummary> {
        self.records
            .values()
            .map(|record| WasmDecoderSummary {
                id: record.manifest.id.clone(),
                name: record.manifest.name.clone(),
                version: record.manifest.version.clone(),
                runtime: record.manifest.runtime.clone(),
                input_topic: record.manifest.input_topic.clone(),
                output_topic: record.manifest.output_topic.clone(),
                module: record.manifest.module.clone(),
                path: record.root.clone(),
                state: record.state,
                decoded_count: record.decoded_count,
                last_error: record.last_error.clone(),
            })
            .collect()
    }

    pub fn count(&self) -> usize {
        self.records.len()
    }

    pub fn process_pending(&mut self) -> usize {
        let mut count = 0;
        for event in self.subscription.drain() {
            count += self.process_event(&event);
        }
        count
    }

    pub fn process_event(&mut self, event: &Event) -> usize {
        let Some(input) = event_payload_bytes(event) else {
            return 0;
        };
        let decoder_ids = self
            .records
            .iter()
            .filter(|(_, record)| {
                record.state == WasmDecoderState::Enabled
                    && record.decoder.is_some()
                    && record.manifest.input_topic == event.topic
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();

        let mut count = 0;
        for decoder_id in decoder_ids {
            match self.decode_one(&decoder_id, event, &input) {
                Ok(true) => count += 1,
                Ok(false) => {}
                Err(error) => self.record_decode_error(&decoder_id, error),
            }
        }
        count
    }

    fn discover_decoder_dir(&mut self, root: &Path) -> WasmHostResult<()> {
        let manifest = load_manifest(&root.join("decoder.json"))?;
        let id = manifest.id.clone();
        let decoded_count = self
            .records
            .get(&id)
            .map(|record| record.decoded_count)
            .unwrap_or_default();
        self.records.insert(
            id,
            WasmDecoderRecord {
                manifest,
                root: root.to_path_buf(),
                decoder: None,
                state: WasmDecoderState::Discovered,
                decoded_count,
                last_error: None,
            },
        );
        Ok(())
    }

    fn decode_one(
        &mut self,
        decoder_id: &str,
        event: &Event,
        input: &[u8],
    ) -> WasmHostResult<bool> {
        let decoded = {
            let record = self
                .records
                .get(decoder_id)
                .ok_or_else(|| WasmHostError::NotFound(decoder_id.to_owned()))?;
            let decoder = record
                .decoder
                .as_ref()
                .ok_or_else(|| WasmHostError::NotFound(decoder_id.to_owned()))?;
            decoder.decode_bytes(input)?
        };

        let Some(value) = decoded else {
            return Ok(false);
        };

        let record = self
            .records
            .get_mut(decoder_id)
            .ok_or_else(|| WasmHostError::NotFound(decoder_id.to_owned()))?;
        record.decoded_count += 1;
        record.last_error = None;

        self.bus.publish(
            Event::json(
                &record.manifest.output_topic,
                format!("wasm:{decoder_id}"),
                value,
            )
            .with_metadata(json!({
                "decoder_id": decoder_id,
                "input_event_id": event.id,
                "input_topic": event.topic,
            })),
        );
        Ok(true)
    }

    fn record_decode_error(&mut self, decoder_id: &str, error: WasmHostError) {
        let error_text = error.to_string();
        if let Some(record) = self.records.get_mut(decoder_id) {
            record.last_error = Some(error_text.clone());
        }
        self.bus.publish(Event::system_log(
            LogLevel::Warn,
            format!("wasm:{decoder_id}"),
            error_text,
        ));
    }
}

fn load_manifest(path: &Path) -> WasmHostResult<WasmDecoderManifest> {
    let text = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

fn register_plugin_host_functions(linker: &mut Linker<PluginHostState>) -> WasmHostResult<()> {
    linker.func_wrap(
        "host",
        "log",
        |mut caller: Caller<'_, PluginHostState>, ptr: i32, len: i32| -> WasmtimeResult<()> {
            ensure_permission(&caller, "log")?;
            let message = read_utf8(&mut caller, ptr, len)?;
            let source = wasm_source(&caller);
            caller
                .data()
                .bus
                .publish(Event::system_log(LogLevel::Info, source, message));
            Ok(())
        },
    )?;

    linker.func_wrap(
        "host",
        "bus_publish",
        |mut caller: Caller<'_, PluginHostState>, ptr: i32, len: i32| -> WasmtimeResult<()> {
            ensure_permission(&caller, "bus")?;
            let bytes = read_memory(&mut caller, ptr, len)?;
            let request: HostBusPublish = serde_json::from_slice(&bytes)?;
            let source = wasm_source(&caller);
            let mut event = Event::json(request.topic, source, request.payload);
            if let Some(metadata) = request.metadata {
                event = event.with_metadata(metadata);
            }
            caller.data().bus.publish(event);
            Ok(())
        },
    )?;

    linker.func_wrap(
        "host",
        "bus_subscribe",
        |mut caller: Caller<'_, PluginHostState>, ptr: i32, len: i32| -> WasmtimeResult<()> {
            ensure_permission(&caller, "bus")?;
            let topic = read_utf8(&mut caller, ptr, len)?;
            caller.data().subscriptions.lock().insert(topic);
            Ok(())
        },
    )?;

    linker.func_wrap(
        "host",
        "ui_panel_create",
        |mut caller: Caller<'_, PluginHostState>, ptr: i32, len: i32| -> WasmtimeResult<()> {
            ensure_permission(&caller, "ui")?;
            let bytes = read_memory(&mut caller, ptr, len)?;
            let request: Value = serde_json::from_slice(&bytes)?;
            let source = wasm_source(&caller);
            caller
                .data()
                .bus
                .publish(Event::json(topics::UI_PANEL_CREATE, source, request));
            Ok(())
        },
    )?;

    linker.func_wrap(
        "host",
        "storage_set",
        |mut caller: Caller<'_, PluginHostState>,
         key_ptr: i32,
         key_len: i32,
         value_ptr: i32,
         value_len: i32|
         -> WasmtimeResult<()> {
            ensure_permission(&caller, "storage")?;
            let key = read_utf8(&mut caller, key_ptr, key_len)?;
            let value = read_utf8(&mut caller, value_ptr, value_len)?;
            caller.data().storage.lock().insert(key, value);
            Ok(())
        },
    )?;

    linker.func_wrap(
        "host",
        "storage_get",
        |mut caller: Caller<'_, PluginHostState>,
         key_ptr: i32,
         key_len: i32,
         out_ptr: i32,
         out_cap: i32|
         -> WasmtimeResult<i32> {
            ensure_permission(&caller, "storage")?;
            let key = read_utf8(&mut caller, key_ptr, key_len)?;
            let Some(value) = caller.data().storage.lock().get(&key).cloned() else {
                return Ok(0);
            };
            let bytes = value.into_bytes();
            if bytes.len() > usize_from_i32(out_cap, "out_cap")? {
                return Ok(-1);
            }
            write_memory(&mut caller, out_ptr, &bytes)?;
            Ok(bytes.len() as i32)
        },
    )?;

    Ok(())
}

#[derive(Debug, Deserialize)]
struct HostBusPublish {
    topic: String,
    #[serde(default)]
    payload: Value,
    #[serde(default)]
    metadata: Option<Value>,
}

fn ensure_permission(caller: &Caller<'_, PluginHostState>, permission: &str) -> WasmtimeResult<()> {
    if !caller.data().permissions.contains(permission) {
        bail!(
            "wasm plugin '{}' does not have '{}' permission",
            caller.data().plugin_id,
            permission
        );
    }
    Ok(())
}

fn read_utf8(
    caller: &mut Caller<'_, PluginHostState>,
    ptr: i32,
    len: i32,
) -> WasmtimeResult<String> {
    let bytes = read_memory(caller, ptr, len)?;
    Ok(String::from_utf8(bytes)?)
}

fn read_memory(
    caller: &mut Caller<'_, PluginHostState>,
    ptr: i32,
    len: i32,
) -> WasmtimeResult<Vec<u8>> {
    let ptr = usize_from_i32(ptr, "ptr")?;
    let len = usize_from_i32(len, "len")?;
    let memory = caller_memory(caller)?;
    let data = memory.data(caller.as_context());
    let end = ptr.checked_add(len).context("memory read overflow")?;
    if end > data.len() {
        bail!(
            "memory read out of bounds: requested {}, available {}",
            end,
            data.len()
        );
    }
    Ok(data[ptr..end].to_vec())
}

fn write_memory(
    caller: &mut Caller<'_, PluginHostState>,
    ptr: i32,
    bytes: &[u8],
) -> WasmtimeResult<()> {
    let ptr = usize_from_i32(ptr, "ptr")?;
    let memory = caller_memory(caller)?;
    let data = memory.data_mut(caller.as_context_mut());
    let end = ptr
        .checked_add(bytes.len())
        .context("memory write overflow")?;
    if end > data.len() {
        bail!(
            "memory write out of bounds: requested {}, available {}",
            end,
            data.len()
        );
    }
    data[ptr..end].copy_from_slice(bytes);
    Ok(())
}

fn caller_memory(caller: &mut Caller<'_, PluginHostState>) -> WasmtimeResult<Memory> {
    caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
        .context("plugin did not export memory")
}

fn usize_from_i32(value: i32, label: &str) -> WasmtimeResult<usize> {
    if value < 0 {
        bail!("{label} was negative");
    }
    Ok(value as usize)
}

fn wasm_source(caller: &Caller<'_, PluginHostState>) -> String {
    format!("wasm:{}", caller.data().plugin_id)
}

fn topic_matches(filter: &str, topic: &str) -> bool {
    if filter == "*" || filter == topic {
        return true;
    }
    filter
        .strip_suffix(".*")
        .is_some_and(|prefix| topic.starts_with(prefix))
}

fn load_module_bytes(path: &Path) -> WasmHostResult<Vec<u8>> {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("wat"))
    {
        return Ok(wat::parse_file(path)?);
    }
    Ok(fs::read(path)?)
}

fn event_payload_bytes(event: &Event) -> Option<Vec<u8>> {
    match &event.payload {
        Payload::Empty => None,
        Payload::Bytes(bytes) => Some(bytes.clone()),
        Payload::Text(text) => Some(text.as_bytes().to_vec()),
        Payload::Json(value) => Some(value.to_string().into_bytes()),
    }
}

fn default_runtime() -> String {
    "wasm".to_owned()
}

fn default_function() -> String {
    "decode".to_owned()
}

fn default_input_topic() -> String {
    topics::SERIAL_RX.to_owned()
}

fn default_output_topic() -> String {
    topics::PROTOCOL_WASM_DECODED.to_owned()
}

fn default_input_ptr() -> u32 {
    0
}

fn default_plugin_input_cap() -> u32 {
    32 * 1024
}

fn default_output_ptr() -> u32 {
    32 * 1024
}

fn default_output_cap() -> u32 {
    8 * 1024
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tool_databus::TopicFilter;

    const HEX_WAT: &str = r#"
(module
  (memory (export "memory") 1)
  (func $hex (param $n i32) (result i32)
    (if (result i32)
      (i32.lt_u (local.get $n) (i32.const 10))
      (then (i32.add (local.get $n) (i32.const 48)))
      (else (i32.add (i32.sub (local.get $n) (i32.const 10)) (i32.const 65)))))
  (func (export "decode") (param $ptr i32) (param $len i32) (param $out i32) (param $cap i32) (result i32)
    (local $b i32)
    (if (i32.or (i32.eqz (local.get $len)) (i32.lt_u (local.get $cap) (i32.const 12)))
      (then (return (i32.const 0))))
    (local.set $b (i32.load8_u (local.get $ptr)))
    (i32.store8 (local.get $out) (i32.const 123))
    (i32.store8 (i32.add (local.get $out) (i32.const 1)) (i32.const 34))
    (i32.store8 (i32.add (local.get $out) (i32.const 2)) (i32.const 104))
    (i32.store8 (i32.add (local.get $out) (i32.const 3)) (i32.const 101))
    (i32.store8 (i32.add (local.get $out) (i32.const 4)) (i32.const 120))
    (i32.store8 (i32.add (local.get $out) (i32.const 5)) (i32.const 34))
    (i32.store8 (i32.add (local.get $out) (i32.const 6)) (i32.const 58))
    (i32.store8 (i32.add (local.get $out) (i32.const 7)) (i32.const 34))
    (i32.store8 (i32.add (local.get $out) (i32.const 8)) (call $hex (i32.shr_u (local.get $b) (i32.const 4))))
    (i32.store8 (i32.add (local.get $out) (i32.const 9)) (call $hex (i32.and (local.get $b) (i32.const 15))))
    (i32.store8 (i32.add (local.get $out) (i32.const 10)) (i32.const 34))
    (i32.store8 (i32.add (local.get $out) (i32.const 11)) (i32.const 125))
    (i32.const 12))
)"#;

    #[test]
    fn decoder_runs_wasm_and_returns_json() {
        let root = create_decoder_dir("direct.decoder");
        let manifest = WasmDecoderManifest {
            id: "direct.decoder".to_owned(),
            name: "Direct Decoder".to_owned(),
            version: "0.1.0".to_owned(),
            runtime: "wasm".to_owned(),
            module: "decoder.wat".to_owned(),
            function: "decode".to_owned(),
            input_topic: topics::SERIAL_RX.to_owned(),
            output_topic: "protocol.wasm.hex".to_owned(),
            input_ptr: 0,
            output_ptr: 32 * 1024,
            output_cap: 1024,
        };
        let decoder = WasmDecoder::load(&root, &manifest, &Engine::default()).unwrap();

        let decoded = decoder.decode_bytes(&[0xAB]).unwrap().unwrap();

        assert_eq!(decoded["hex"], "AB");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manager_publishes_decoded_events_to_databus() {
        let parent = create_decoder_root("manager.decoder");
        let bus = DataBus::new();
        let decoded = bus.subscribe(TopicFilter::exact("protocol.wasm.hex"));
        let mut manager = WasmDecoderManager::new(bus.clone());

        assert_eq!(manager.discover_roots([parent.clone()]).unwrap(), 1);
        manager.enable("manager.decoder").unwrap();
        bus.publish(Event::serial_rx("test", vec![0x5A]));

        assert_eq!(manager.process_pending(), 1);
        let event = decoded.recv_timeout(Duration::from_millis(250)).unwrap();
        assert_eq!(event.source, "wasm:manager.decoder");
        assert_eq!(event.metadata["decoder_id"], "manager.decoder");
        match event.payload {
            Payload::Json(value) => assert_eq!(value["hex"], "5A"),
            payload => panic!("unexpected payload: {payload:?}"),
        }
        let _ = fs::remove_dir_all(parent);
    }

    fn create_decoder_root(id: &str) -> PathBuf {
        let parent = std::env::temp_dir().join(format!(
            "hardware-workbench-wasm-test-{}-{}-{}",
            id,
            std::process::id(),
            tool_core::now_timestamp_ms()
        ));
        let decoder_dir = parent.join(id);
        fs::create_dir_all(&decoder_dir).unwrap();
        write_decoder_files(&decoder_dir, id);
        parent
    }

    fn create_decoder_dir(id: &str) -> PathBuf {
        let parent = create_decoder_root(id);
        parent.join(id)
    }

    fn write_decoder_files(root: &Path, id: &str) {
        fs::write(root.join("decoder.wat"), HEX_WAT).unwrap();
        fs::write(
            root.join("decoder.json"),
            format!(
                r#"{{
  "id": "{id}",
  "name": "HEX Byte Decoder",
  "version": "0.1.0",
  "runtime": "wasm",
  "module": "decoder.wat",
  "input_topic": "transport.serial.default.rx",
  "output_topic": "protocol.wasm.hex",
  "function": "decode",
  "input_ptr": 0,
  "output_ptr": 32768,
  "output_cap": 1024
}}"#
            ),
        )
        .unwrap();
    }
}

use std::rc::Rc;

use crate::{PluginHostApi, PluginPermissions, PluginResult, PluginValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct PluginInstanceId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PluginFunctionId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CoroutineId(pub u64);

#[derive(Debug, Clone, PartialEq)]
pub struct PluginLoadConfig {
    pub plugin_id: String,
    pub plugin_name: String,
    pub plugin_version: String,
    pub script_name: String,
    pub context: PluginValue,
    pub permissions: PluginPermissions,
}

/// Explicit suspension requests shared by Native and Web schedulers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginYield {
    SleepMs(u64),
    WaitBus {
        topic: String,
        timeout_ms: Option<u64>,
    },
    WaitSerialLine {
        port: String,
        timeout_ms: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum PluginCallResult {
    Completed(PluginValue),
    Yielded {
        coroutine: CoroutineId,
        request: PluginYield,
    },
}

/// Engine boundary. No `mlua::Value` or VM-specific table/function handle may
/// cross this trait.
pub trait LuaEngine {
    fn load_plugin(
        &mut self,
        source: &str,
        config: PluginLoadConfig,
        host: Rc<dyn PluginHostApi>,
    ) -> PluginResult<PluginInstanceId>;

    fn call(
        &mut self,
        instance: PluginInstanceId,
        function: PluginFunctionId,
        args: &[PluginValue],
    ) -> PluginResult<PluginCallResult>;

    fn resume(
        &mut self,
        coroutine: CoroutineId,
        value: PluginValue,
    ) -> PluginResult<PluginCallResult>;

    fn stop(&mut self, instance: PluginInstanceId) -> PluginResult<()>;

    fn dispatch_event(
        &mut self,
        _instance: PluginInstanceId,
        _event: PluginValue,
    ) -> PluginResult<()> {
        Err(crate::PluginError::UnsupportedCapability(
            "bus.subscribe".to_owned(),
        ))
    }

    fn dispatch_command(
        &mut self,
        _instance: PluginInstanceId,
        _command: &str,
        _context: PluginValue,
    ) -> PluginResult<PluginCallResult> {
        Err(crate::PluginError::UnsupportedCapability(
            "commands".to_owned(),
        ))
    }

    fn update_settings(
        &mut self,
        _instance: PluginInstanceId,
        _settings: PluginValue,
    ) -> PluginResult<()> {
        Err(crate::PluginError::UnsupportedCapability(
            "config".to_owned(),
        ))
    }
}

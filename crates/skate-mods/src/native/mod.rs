//! Built-in Rust ports of Lua packages, selected by mod ID. The web build has no
//! Lua runtime, so these stand in for the scripts there; natively they run only
//! when `SKATE3_NATIVE_MODS=1`, so both can be compared on the same inputs.
use crate::Command;
use serde_json::Value;
use std::collections::BTreeMap;

mod freestyle_mx;

pub(crate) struct Context<'a> {
    pub mod_id: &'a str,
    pub snapshot: &'a Value,
    pub settings: &'a BTreeMap<String, Value>,
}

pub(crate) trait Script: Send + Sync {
    fn call(&mut self, name: &str, payload: &Value, ctx: &Context) -> Result<Vec<Command>, String>;
}

pub(crate) fn find(mod_id: &str) -> Option<Box<dyn Script>> {
    match mod_id {
        "community.freestyle-mx" => Some(Box::<freestyle_mx::FreestyleMx>::default()),
        _ => None,
    }
}

pub(crate) fn preferred() -> bool {
    cfg!(target_arch = "wasm32") || std::env::var_os("SKATE3_NATIVE_MODS").is_some_and(|v| v == "1")
}

/// A package driven by a built-in script, with the same limits as a Lua one.
pub(crate) struct NativeVm {
    id: String,
    script: Box<dyn Script>,
    settings: BTreeMap<String, Value>,
}
impl NativeVm {
    pub fn new(id: &str, settings: &BTreeMap<String, Value>) -> Option<Self> {
        find(id).map(|script| Self { id: id.to_owned(), script, settings: settings.clone() })
    }
    pub fn settings(&mut self, settings: &BTreeMap<String, Value>) {
        self.settings = settings.clone();
    }
    pub fn call(&mut self, name: &str, payload: Value, snapshot: &Value) -> Result<Vec<Command>, String> {
        let ctx = Context { mod_id: &self.id, snapshot, settings: &self.settings };
        let commands = self.script.call(name, &payload, &ctx)?;
        if commands.len() > 128 {
            return Err("128 commands per callback maximum".into());
        }
        if !commands.iter().all(Command::validate) {
            return Err("Invalid command arguments".into());
        }
        Ok(commands)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod parity;

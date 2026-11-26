use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use rusqlite::InterruptHandle;

type RegistryKey = (String, u32);

static REGISTRY: OnceLock<Mutex<HashMap<RegistryKey, Arc<InterruptHandle>>>> = OnceLock::new();
static NEXT_CONNECTION_ID: AtomicU32 = AtomicU32::new(1);

fn registry() -> &'static Mutex<HashMap<RegistryKey, Arc<InterruptHandle>>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn register(profile_id: &str, handle: InterruptHandle) -> u32 {
    let connection_id = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut entries) = registry().lock() {
        entries.insert((profile_id.to_string(), connection_id), Arc::new(handle));
    }
    connection_id
}

pub(crate) fn unregister(profile_id: &str, connection_id: u32) {
    if let Ok(mut entries) = registry().lock() {
        entries.remove(&(profile_id.to_string(), connection_id));
    }
}

pub(crate) fn interrupt(profile_id: &str, connection_id: u32) -> bool {
    let handle = registry()
        .lock()
        .ok()
        .and_then(|entries| entries.get(&(profile_id.to_string(), connection_id)).cloned());
    match handle {
        Some(handle) => {
            handle.interrupt();
            true
        }
        None => false,
    }
}

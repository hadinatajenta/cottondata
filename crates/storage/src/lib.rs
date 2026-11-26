mod sessions;

pub use sessions::{
    now_secs, ConnectedServerState, SessionSortOrder, SessionTabState, SessionsStore, WorkspaceSession,
    DEFAULT_SESSION_ID, MAX_SESSIONS,
};

use cottondata_core::{ServerProfile, ServerRegistry, DEFAULT_MAX_SERVERS, SERVER_SLOT_CAPACITY};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_HISTORY_ENTRIES: usize = 300;
const KEYCHAIN_SERVICE: &str = "dev.cottondata.mysql";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryHistoryItem {
    pub timestamp_secs: u64,
    pub server_name: String,
    pub schema: Option<String>,
    pub sql: String,
    pub elapsed_ms: u64,
    pub succeeded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProfilesEnvelope {
    version: u8,
    #[serde(default = "default_max_servers")]
    max_servers: usize,
    profiles: Vec<ServerProfile>,
}

fn default_max_servers() -> usize {
    DEFAULT_MAX_SERVERS
}

pub struct AppStorage {
    data_dir: PathBuf,
    session_passwords: HashMap<String, String>,
}

impl AppStorage {
    pub fn open_default() -> io::Result<Self> {
        let dir = default_data_dir();
        fs::create_dir_all(&dir)?;
        Ok(Self {
            data_dir: dir,
            session_passwords: HashMap::new(),
        })
    }

    pub fn open_at(data_dir: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&data_dir)?;
        Ok(Self {
            data_dir,
            session_passwords: HashMap::new(),
        })
    }

    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }

    fn profiles_path(&self) -> PathBuf {
        self.data_dir.join("servers.json")
    }

    fn history_path(&self) -> PathBuf {
        self.data_dir.join("history.json")
    }

    fn sessions_path(&self) -> PathBuf {
        self.data_dir.join("sessions.json")
    }

    pub fn load_sessions(&self) -> SessionsStore {
        let path = self.sessions_path();
        let Ok(raw) = fs::read_to_string(path) else {
            return SessionsStore::default();
        };
        let mut store: SessionsStore = serde_json::from_str(&raw).unwrap_or_default();
        store.ensure_valid();
        store
    }

    pub fn save_sessions(&self, store: &SessionsStore) -> io::Result<()> {
        let payload = serde_json::to_string_pretty(store).map_err(io::Error::other)?;
        let path = self.sessions_path();
        let tmp = self.data_dir.join("sessions.json.tmp");
        fs::write(&tmp, payload)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
        }
        fs::rename(tmp, path)
    }

    pub fn load_registry(&self) -> io::Result<ServerRegistry> {
        let path = self.profiles_path();
        if !path.exists() {
            return Ok(ServerRegistry::new());
        }
        let raw = fs::read_to_string(&path)?;
        let env: ProfilesEnvelope = serde_json::from_str(&raw)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let profiles: Vec<ServerProfile> = env.profiles.into_iter().take(SERVER_SLOT_CAPACITY).collect();
        ServerRegistry::from_profiles(profiles, env.max_servers)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
    }

    pub fn save_registry(&self, registry: &ServerRegistry) -> io::Result<()> {
        let env = ProfilesEnvelope {
            version: 1,
            max_servers: registry.max_servers(),
            profiles: registry.profiles(),
        };
        let payload = serde_json::to_string_pretty(&env)
            .map_err(io::Error::other)?;
        let path = self.profiles_path();
        let tmp = self.data_dir.join("servers.json.tmp");
        fs::write(&tmp, payload)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
        }
        fs::rename(tmp, path)
    }

    pub fn load_history(&self) -> Vec<QueryHistoryItem> {
        let path = self.history_path();
        let Ok(raw) = fs::read_to_string(path) else {
            return Vec::new();
        };
        serde_json::from_str(&raw).unwrap_or_default()
    }

    pub fn record_history(
        &self,
        server_name: impl Into<String>,
        schema: Option<String>,
        sql: impl Into<String>,
        elapsed_ms: u64,
        succeeded: bool,
    ) -> Vec<QueryHistoryItem> {
        let mut items = self.load_history();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        items.insert(
            0,
            QueryHistoryItem {
                timestamp_secs: now,
                server_name: server_name.into(),
                schema,
                sql: sql.into(),
                elapsed_ms,
                succeeded,
            },
        );
        items.truncate(MAX_HISTORY_ENTRIES);
        if let Ok(json) = serde_json::to_string(&items) {
            let _ = fs::write(self.history_path(), json);
        }
        items
    }

    pub fn save_password(&mut self, profile_id: &str, password: &str, persist_os_vault: bool) {
        self.session_passwords
            .insert(profile_id.to_string(), password.to_string());
        if persist_os_vault {
            #[cfg(target_os = "macos")]
            {
                let _ = Command::new("security")
                    .args([
                        "add-generic-password",
                        "-U",
                        "-a",
                        profile_id,
                        "-s",
                        KEYCHAIN_SERVICE,
                        "-w",
                        password,
                    ])
                    .status();
            }
        }
    }

    pub fn get_password(&mut self, profile_id: &str) -> Option<String> {
        if let Some(pw) = self.session_passwords.get(profile_id) {
            return Some(pw.clone());
        }
        #[cfg(target_os = "macos")]
        {
            if let Ok(out) = Command::new("security")
                .args([
                    "find-generic-password",
                    "-a",
                    profile_id,
                    "-s",
                    KEYCHAIN_SERVICE,
                    "-w",
                ])
                .output()
            {
                if out.status.success() {
                    let pw = String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string();
                    self.session_passwords.insert(profile_id.to_string(), pw.clone());
                    return Some(pw);
                }
            }
        }
        None
    }

    pub fn delete_password(&mut self, profile_id: &str) {
        self.session_passwords.remove(profile_id);
        #[cfg(target_os = "macos")]
        {
            let _ = Command::new("security")
                .args([
                    "delete-generic-password",
                    "-a",
                    profile_id,
                    "-s",
                    KEYCHAIN_SERVICE,
                ])
                .status();
        }
    }
}

pub fn default_data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("CottonData");
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("CottonData");
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("cottondata");
            }
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".config").join("cottondata");
        }
    }
    PathBuf::from(".cottondata")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_reloads_profiles_within_ten_slot_cap() {
        let dir = std::env::temp_dir().join(format!(
            "cottondata-storage-test-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let storage = AppStorage::open_at(dir.clone()).unwrap();
        let mut reg = ServerRegistry::new();
        reg.add(ServerProfile::new_local("Local MySQL", "127.0.0.1", 3306, "root"))
            .unwrap();
        storage.save_registry(&reg).unwrap();

        let loaded = storage.load_registry().unwrap();
        assert_eq!(loaded.count(), 1);
        assert_eq!(loaded.profiles()[0].name, "Local MySQL");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn persists_and_reloads_named_sessions_tabs_and_connections() {
        let dir = std::env::temp_dir().join(format!(
            "cottondata-sessions-test-{}",
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let storage = AppStorage::open_at(dir.clone()).unwrap();

        let mut store = storage.load_sessions();
        assert_eq!(store.active_session().name, "Default");
        store.dark_theme = true;
        store.show_system_schemas = true;

        {
            let active = store.active_session_mut();
            active.active_server_id = Some("srv-prod-1".into());
            active.tabs.push(SessionTabState {
                id: 1,
                title: "Console 1".into(),
                server_id: Some("srv-prod-1".into()),
                sql: "SELECT * FROM orders LIMIT 10;".into(),
                cursor_byte: 12,
                bound_table: None,
                where_clause: String::new(),
                order_by: None,
                page_index: 0,
                page_size: 500,
                show_sql_editor: Some(true),
            });
            active.tabs.push(SessionTabState {
                id: 2,
                title: "shop.orders".into(),
                server_id: Some("srv-prod-1".into()),
                sql: "SELECT * FROM `shop`.`orders` LIMIT 500 OFFSET 0;".into(),
                cursor_byte: 0,
                bound_table: Some(("shop".into(), "orders".into())),
                where_clause: "status = 'paid'".into(),
                order_by: Some(("id".into(), SessionSortOrder::Desc)),
                page_index: 2,
                page_size: 500,
                show_sql_editor: Some(false),
            });
            active.editor_height = 360;
            active.connected_servers.push(ConnectedServerState {
                server_id: "srv-prod-1".into(),
                active_schema: Some("shop".into()),
                autocommit: false,
            });
            active.active_tab_idx = 1;
            active.next_tab_id = 3;
        }

        let dup_id = store.duplicate_active_session("Analytics").unwrap();
        assert_eq!(store.active_session_id, dup_id);
        assert_eq!(store.sessions.len(), 2);
        assert_eq!(store.active_session().tabs.len(), 2);

        store.rename_session(&dup_id, "Analytics Prod").unwrap();
        storage.save_sessions(&store).unwrap();

        let mut reloaded = storage.load_sessions();
        assert!(reloaded.dark_theme);
        assert!(reloaded.show_system_schemas);
        assert_eq!(reloaded.active_session().name, "Analytics Prod");
        assert_eq!(reloaded.active_session().tabs[0].sql, "SELECT * FROM orders LIMIT 10;");
        assert_eq!(
            reloaded.active_session().tabs[1].order_by,
            Some(("id".into(), SessionSortOrder::Desc))
        );
        assert_eq!(reloaded.active_session().connected_servers.len(), 1);
        assert_eq!(
            reloaded.active_session().connected_servers[0].active_schema.as_deref(),
            Some("shop")
        );
        assert!(!reloaded.active_session().connected_servers[0].autocommit);

        assert_eq!(reloaded.active_session().editor_height, 360);
        assert_eq!(reloaded.active_session().tabs[0].show_sql_editor, Some(true));
        assert_eq!(reloaded.active_session().tabs[1].show_sql_editor, Some(false));

        let legacy_json = r#"{"version":1,"active_session_id":"default","sessions":[{"id":"default","name":"Default","active_server_id":null,"active_tab_idx":0,"next_tab_id":2,"tabs":[{"id":1,"title":"Console 1","server_id":null,"sql":"SELECT 1;","cursor_byte":0,"bound_table":null,"where_clause":"","order_by":null,"page_index":0,"page_size":500}],"connected_servers":[],"updated_at_secs":1}]}"#;
        let legacy: SessionsStore = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(legacy.active_session().editor_height, 220);
        assert_eq!(legacy.active_session().tabs[0].show_sql_editor, None);

        reloaded.delete_session(&dup_id).unwrap();
        assert_eq!(reloaded.sessions.len(), 1);
        assert!(reloaded.delete_session(DEFAULT_SESSION_ID).is_err());

        let _ = fs::remove_dir_all(dir);
    }
}

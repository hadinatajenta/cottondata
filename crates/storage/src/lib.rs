use cottondata_core::{ServerProfile, ServerRegistry, MAX_SERVERS};
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
    max_servers: usize,
    profiles: Vec<ServerProfile>,
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

    pub fn load_registry(&self) -> io::Result<ServerRegistry> {
        let path = self.profiles_path();
        if !path.exists() {
            return Ok(ServerRegistry::new());
        }
        let raw = fs::read_to_string(&path)?;
        let env: ProfilesEnvelope = serde_json::from_str(&raw)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let profiles: Vec<ServerProfile> = env.profiles.into_iter().take(MAX_SERVERS).collect();
        ServerRegistry::from_profiles(profiles)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
    }

    pub fn save_registry(&self, registry: &ServerRegistry) -> io::Result<()> {
        let env = ProfilesEnvelope {
            version: 1,
            max_servers: MAX_SERVERS,
            profiles: registry.profiles(),
        };
        let payload = serde_json::to_string_pretty(&env)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
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
}

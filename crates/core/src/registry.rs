use crate::dialect::DatabaseEngine;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const SERVER_SLOT_CAPACITY: usize = 64;
pub const DEFAULT_MAX_SERVERS: usize = 10;
pub const MAX_SOCKETS_PER_SERVER: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ServerSlotId(u8);

impl ServerSlotId {
    pub fn new(index: usize) -> Option<Self> {
        if index < SERVER_SLOT_CAPACITY {
            Some(Self(index as u8))
        } else {
            None
        }
    }

    pub fn index(self) -> usize {
        self.0 as usize
    }

    pub fn display_number(self) -> usize {
        self.index() + 1
    }

    pub fn shortcut_digit(self) -> char {
        match self.0 {
            0..=8 => char::from(b'1' + self.0),
            _ => '0',
        }
    }
}

impl fmt::Display for ServerSlotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.display_number())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EnvironmentTag {
    #[default]
    Local,
    Development,
    Staging,
    Production,
}

impl EnvironmentTag {
    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "LOCAL",
            Self::Development => "DEV",
            Self::Staging => "STAGING",
            Self::Production => "PROD",
        }
    }

    pub fn rgb(self) -> [u8; 3] {
        match self {
            Self::Local => [82, 196, 152],
            Self::Development => [92, 168, 255],
            Self::Staging => [244, 184, 72],
            Self::Production => [242, 96, 108],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TlsMode {
    Disabled,
    #[default]
    Preferred,
    Required,
    VerifyCa,
    VerifyIdentity,
}

impl TlsMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disabled => "Disabled",
            Self::Preferred => "Preferred",
            Self::Required => "Required",
            Self::VerifyCa => "Verify CA",
            Self::VerifyIdentity => "Verify Identity",
        }
    }

    pub fn all() -> &'static [TlsMode] {
        &[
            Self::Disabled,
            Self::Preferred,
            Self::Required,
            Self::VerifyCa,
            Self::VerifyIdentity,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionEndpoint {
    Tcp { host: String, port: u16 },
    UnixSocket { path: String },
}

impl Default for ConnectionEndpoint {
    fn default() -> Self {
        Self::Tcp {
            host: "127.0.0.1".to_string(),
            port: 3306,
        }
    }
}

impl ConnectionEndpoint {
    pub fn display_address(&self) -> String {
        match self {
            Self::Tcp { host, port } => format!("{host}:{port}"),
            Self::UnixSocket { path } => format!("unix://{path}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SshTunnelConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub identity_file: Option<String>,
    pub local_bind_port: Option<u16>,
}

impl SshTunnelConfig {
    pub fn default_enabled() -> Self {
        Self {
            enabled: false,
            host: String::new(),
            port: 22,
            username: String::new(),
            identity_file: None,
            local_bind_port: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub engine: DatabaseEngine,
    pub endpoint: ConnectionEndpoint,
    pub username: String,
    #[serde(default)]
    pub default_schema: Option<String>,
    #[serde(default)]
    pub tls: TlsMode,
    #[serde(default)]
    pub ca_cert_path: Option<String>,
    #[serde(default)]
    pub ssh: SshTunnelConfig,
    #[serde(default)]
    pub environment: EnvironmentTag,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default = "default_page_size")]
    pub page_size: u32,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_secs: u16,
    #[serde(default = "default_idle_disconnect_mins")]
    pub idle_disconnect_mins: u16,
    #[serde(default)]
    pub show_system_schemas: bool,
}

fn default_page_size() -> u32 {
    500
}

fn default_connect_timeout() -> u16 {
    10
}

fn default_idle_disconnect_mins() -> u16 {
    15
}

impl ServerProfile {
    pub fn new_local(name: impl Into<String>, host: impl Into<String>, port: u16, user: impl Into<String>) -> Self {
        let name_str = name.into();
        let slug = name_str
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
            .collect::<String>();
        Self {
            id: format!("srv-{slug}-{port}"),
            name: name_str,
            engine: DatabaseEngine::MySql,
            endpoint: ConnectionEndpoint::Tcp {
                host: host.into(),
                port,
            },
            username: user.into(),
            default_schema: None,
            tls: TlsMode::Preferred,
            ca_cert_path: None,
            ssh: SshTunnelConfig::default_enabled(),
            environment: EnvironmentTag::Local,
            read_only: false,
            page_size: default_page_size(),
            connect_timeout_secs: default_connect_timeout(),
            idle_disconnect_mins: default_idle_disconnect_mins(),
            show_system_schemas: false,
        }
    }

    pub fn validate(&self) -> Result<(), RegistryError> {
        if self.name.trim().is_empty() {
            return Err(RegistryError::InvalidProfile("Server name cannot be empty".into()));
        }
        if self.username.trim().is_empty() {
            return Err(RegistryError::InvalidProfile("MySQL username cannot be empty".into()));
        }
        match &self.endpoint {
            ConnectionEndpoint::Tcp { host, port } => {
                if host.trim().is_empty() {
                    return Err(RegistryError::InvalidProfile("MySQL host cannot be empty".into()));
                }
                if *port == 0 {
                    return Err(RegistryError::InvalidProfile("MySQL port must be 1..=65535".into()));
                }
            }
            ConnectionEndpoint::UnixSocket { path } => {
                if path.trim().is_empty() {
                    return Err(RegistryError::InvalidProfile("Unix socket path cannot be empty".into()));
                }
            }
        }
        if self.ssh.enabled && (self.ssh.host.trim().is_empty() || self.ssh.username.trim().is_empty()) {
            return Err(RegistryError::InvalidProfile(
                "SSH tunnel requires both SSH host and username".into(),
            ));
        }
        if self.page_size == 0 || self.page_size > 10_000 {
            return Err(RegistryError::InvalidProfile("Page size must be between 1 and 10,000".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ConnectionStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerRuntimeInfo {
    pub version: String,
    pub version_major: u16,
    pub version_minor: u16,
    pub version_patch: u16,
    pub lower_case_table_names: u8,
    pub sql_mode: String,
    pub time_zone: String,
    pub current_schema: Option<String>,
    pub connection_id: u32,
    pub autocommit: bool,
    pub in_transaction: bool,
}

impl ServerRuntimeInfo {
    pub fn supports_explain_analyze(&self) -> bool {
        (self.version_major, self.version_minor, self.version_patch) >= (8, 0, 18)
    }

    pub fn supports_functional_indexes(&self) -> bool {
        (self.version_major, self.version_minor, self.version_patch) >= (8, 0, 13)
    }

    pub fn supports_check_constraints(&self) -> bool {
        (self.version_major, self.version_minor, self.version_patch) >= (8, 0, 16)
    }

    pub fn supports_generated_columns(&self) -> bool {
        (self.version_major, self.version_minor) >= (5, 7)
    }

    pub fn parse_version_tuple(version: &str) -> (u16, u16, u16) {
        let numeric = version.split('-').next().unwrap_or(version);
        let mut parts = numeric.split('.').map(|s| s.parse::<u16>().unwrap_or(0));
        (
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
        )
    }
}

#[derive(Debug, Clone)]
pub struct ServerSlot {
    pub slot_id: ServerSlotId,
    pub profile: ServerProfile,
    pub status: ConnectionStatus,
    pub runtime: Option<ServerRuntimeInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryError {
    SlotLimitReached { max: usize },
    SlotEmpty(ServerSlotId),
    DuplicateName(String),
    InvalidProfile(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SlotLimitReached { max } => {
                write!(f, "Maximum of {max} MySQL server slots reached; remove a server before adding another")
            }
            Self::SlotEmpty(id) => write!(f, "Server slot {id} is empty"),
            Self::DuplicateName(name) => write!(f, "A server named '{name}' already exists"),
            Self::InvalidProfile(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for RegistryError {}

#[derive(Debug, Clone)]
pub struct ServerRegistry {
    slots: [Option<ServerSlot>; SERVER_SLOT_CAPACITY],
    max_servers: usize,
}

impl Default for ServerRegistry {
    fn default() -> Self {
        Self {
            slots: std::array::from_fn(|_| None),
            max_servers: DEFAULT_MAX_SERVERS,
        }
    }
}

impl ServerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    pub fn max_servers(&self) -> usize {
        self.max_servers
    }

    pub fn set_max_servers(&mut self, max_servers: usize) -> Result<(), RegistryError> {
        if !(1..=SERVER_SLOT_CAPACITY).contains(&max_servers) {
            return Err(RegistryError::InvalidProfile(format!(
                "Maximum servers must be between 1 and {SERVER_SLOT_CAPACITY}"
            )));
        }
        if max_servers < self.count() {
            return Err(RegistryError::InvalidProfile(format!(
                "Remove servers first: {} are configured, which is more than {max_servers}",
                self.count()
            )));
        }
        self.max_servers = max_servers;
        Ok(())
    }

    pub fn remaining_slots(&self) -> usize {
        self.max_servers.saturating_sub(self.count())
    }

    pub fn is_full(&self) -> bool {
        self.count() >= self.max_servers
    }

    pub fn add(&mut self, profile: ServerProfile) -> Result<ServerSlotId, RegistryError> {
        profile.validate()?;
        if self.slots.iter().flatten().any(|s| s.profile.name.eq_ignore_ascii_case(&profile.name)) {
            return Err(RegistryError::DuplicateName(profile.name));
        }
        if self.is_full() {
            return Err(RegistryError::SlotLimitReached { max: self.max_servers });
        }
        let free_idx = self
            .slots
            .iter()
            .position(|slot| slot.is_none())
            .ok_or(RegistryError::SlotLimitReached { max: self.max_servers })?;
        let slot_id = ServerSlotId::new(free_idx).expect("index within SERVER_SLOT_CAPACITY");
        self.slots[free_idx] = Some(ServerSlot {
            slot_id,
            profile,
            status: ConnectionStatus::Disconnected,
            runtime: None,
        });
        Ok(slot_id)
    }

    pub fn update(&mut self, slot_id: ServerSlotId, profile: ServerProfile) -> Result<(), RegistryError> {
        profile.validate()?;
        for (idx, existing) in self.slots.iter().enumerate() {
            if idx != slot_id.index() {
                if let Some(slot) = existing {
                    if slot.profile.name.eq_ignore_ascii_case(&profile.name) {
                        return Err(RegistryError::DuplicateName(profile.name));
                    }
                }
            }
        }
        let slot = self.slots[slot_id.index()]
            .as_mut()
            .ok_or(RegistryError::SlotEmpty(slot_id))?;
        slot.profile = profile;
        Ok(())
    }

    pub fn remove(&mut self, slot_id: ServerSlotId) -> Result<ServerSlot, RegistryError> {
        self.slots[slot_id.index()]
            .take()
            .ok_or(RegistryError::SlotEmpty(slot_id))
    }

    pub fn get(&self, slot_id: ServerSlotId) -> Option<&ServerSlot> {
        self.slots[slot_id.index()].as_ref()
    }

    pub fn get_mut(&mut self, slot_id: ServerSlotId) -> Option<&mut ServerSlot> {
        self.slots[slot_id.index()].as_mut()
    }

    pub fn iter(&self) -> impl Iterator<Item = &ServerSlot> {
        self.slots.iter().flatten()
    }

    pub fn profiles(&self) -> Vec<ServerProfile> {
        self.iter().map(|s| s.profile.clone()).collect()
    }

    pub fn from_profiles(profiles: Vec<ServerProfile>, max_servers: usize) -> Result<Self, RegistryError> {
        let mut reg = Self::new();
        reg.max_servers = max_servers.clamp(1, SERVER_SLOT_CAPACITY).max(profiles.len().min(SERVER_SLOT_CAPACITY));
        if profiles.len() > SERVER_SLOT_CAPACITY {
            return Err(RegistryError::SlotLimitReached { max: SERVER_SLOT_CAPACITY });
        }
        for profile in profiles {
            reg.add(profile)?;
        }
        Ok(reg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_strict_ten_server_cap() {
        let mut reg = ServerRegistry::new();
        for i in 1..=DEFAULT_MAX_SERVERS {
            let p = ServerProfile::new_local(format!("MySQL-{i}"), "127.0.0.1", 3306 + i as u16, "root");
            let id = reg.add(p).expect("slot should succeed");
            assert_eq!(id.display_number(), i);
        }
        assert!(reg.is_full());
        assert_eq!(reg.remaining_slots(), 0);

        let eleventh = ServerProfile::new_local("MySQL-11", "127.0.0.1", 3399, "root");
        assert_eq!(
            reg.add(eleventh),
            Err(RegistryError::SlotLimitReached { max: 10 })
        );
    }

    #[test]
    fn user_limit_can_grow_up_to_capacity_but_not_below_configured_count() {
        let mut reg = ServerRegistry::new();
        for i in 1..=3 {
            reg.add(ServerProfile::new_local(format!("db-{i}"), "127.0.0.1", 3306, "root"))
                .unwrap();
        }
        assert!(reg.set_max_servers(2).is_err());
        assert!(reg.set_max_servers(0).is_err());
        assert!(reg.set_max_servers(SERVER_SLOT_CAPACITY + 1).is_err());
        reg.set_max_servers(3).unwrap();
        assert!(reg.is_full());
        reg.set_max_servers(SERVER_SLOT_CAPACITY).unwrap();
        assert_eq!(reg.remaining_slots(), SERVER_SLOT_CAPACITY - 3);
    }

    #[test]
    fn loading_more_profiles_than_saved_limit_raises_limit_instead_of_dropping_servers() {
        let profiles = (1..=5)
            .map(|i| ServerProfile::new_local(format!("db-{i}"), "127.0.0.1", 3306, "root"))
            .collect();
        let reg = ServerRegistry::from_profiles(profiles, 2).unwrap();
        assert_eq!(reg.count(), 5);
        assert_eq!(reg.max_servers(), 5);
    }

    #[test]
    fn rejects_duplicate_names_case_insensitively() {
        let mut reg = ServerRegistry::new();
        reg.add(ServerProfile::new_local("Prod-Primary", "10.0.0.1", 3306, "app"))
            .unwrap();
        let dup = ServerProfile::new_local("prod-primary", "10.0.0.2", 3306, "app");
        assert!(matches!(reg.add(dup), Err(RegistryError::DuplicateName(_))));
    }

    #[test]
    fn version_capability_checks_match_mysql_milestones() {
        let (maj, min, pat) = ServerRuntimeInfo::parse_version_tuple("8.0.36-commercial");
        let info = ServerRuntimeInfo {
            version: "8.0.36-commercial".into(),
            version_major: maj,
            version_minor: min,
            version_patch: pat,
            ..Default::default()
        };
        assert!(info.supports_explain_analyze());
        assert!(info.supports_functional_indexes());
        assert!(info.supports_check_constraints());
        assert!(info.supports_generated_columns());

        let old = ServerRuntimeInfo {
            version_major: 5,
            version_minor: 7,
            version_patch: 44,
            ..Default::default()
        };
        assert!(!old.supports_explain_analyze());
        assert!(!old.supports_functional_indexes());
        assert!(old.supports_generated_columns());
    }
}

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_SESSION_ID: &str = "default";
pub const MAX_SESSIONS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionSortOrder {
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionTabState {
    pub id: u64,
    pub title: String,
    pub server_id: Option<String>,
    pub sql: String,
    pub cursor_byte: usize,
    pub bound_table: Option<(String, String)>,
    pub where_clause: String,
    pub order_by: Option<(String, SessionSortOrder)>,
    pub page_index: u64,
    pub page_size: u32,
    #[serde(default)]
    pub show_sql_editor: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectedServerState {
    pub server_id: String,
    pub active_schema: Option<String>,
    #[serde(default = "default_autocommit")]
    pub autocommit: bool,
}

fn default_autocommit() -> bool {
    true
}

fn default_editor_height() -> u32 {
    220
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSession {
    pub id: String,
    pub name: String,
    pub active_server_id: Option<String>,
    pub active_tab_idx: usize,
    pub next_tab_id: u64,
    pub tabs: Vec<SessionTabState>,
    pub connected_servers: Vec<ConnectedServerState>,
    pub updated_at_secs: u64,
    #[serde(default = "default_editor_height")]
    pub editor_height: u32,
}

impl WorkspaceSession {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            active_server_id: None,
            active_tab_idx: 0,
            next_tab_id: 2,
            tabs: Vec::new(),
            connected_servers: Vec::new(),
            updated_at_secs: now_secs(),
            editor_height: default_editor_height(),
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionsStore {
    pub version: u8,
    pub active_session_id: String,
    #[serde(default)]
    pub dark_theme: bool,
    #[serde(default)]
    pub show_system_schemas: bool,
    #[serde(default = "default_true")]
    pub auto_check_updates: bool,
    pub sessions: Vec<WorkspaceSession>,
}

impl Default for SessionsStore {
    fn default() -> Self {
        Self {
            version: 1,
            active_session_id: DEFAULT_SESSION_ID.to_string(),
            dark_theme: false,
            show_system_schemas: false,
            auto_check_updates: true,
            sessions: vec![WorkspaceSession::new(DEFAULT_SESSION_ID, "Default")],
        }
    }
}

impl SessionsStore {
    pub fn ensure_valid(&mut self) {
        if self.version == 0 {
            self.version = 1;
        }
        if self.sessions.is_empty() {
            self.sessions
                .push(WorkspaceSession::new(DEFAULT_SESSION_ID, "Default"));
        }
        if !self.sessions.iter().any(|s| s.id == self.active_session_id) {
            self.active_session_id = self.sessions[0].id.clone();
        }
    }

    pub fn active_session(&self) -> &WorkspaceSession {
        self.sessions
            .iter()
            .find(|s| s.id == self.active_session_id)
            .unwrap_or(&self.sessions[0])
    }

    pub fn active_session_mut(&mut self) -> &mut WorkspaceSession {
        self.ensure_valid();
        let idx = self
            .sessions
            .iter()
            .position(|s| s.id == self.active_session_id)
            .unwrap_or(0);
        &mut self.sessions[idx]
    }

    pub fn select_session(&mut self, id: &str) -> bool {
        if self.sessions.iter().any(|s| s.id == id) {
            self.active_session_id = id.to_string();
            true
        } else {
            false
        }
    }

    pub fn create_session(&mut self, name: impl Into<String>) -> Result<String, String> {
        let trimmed = name.into().trim().to_string();
        if trimmed.is_empty() {
            return Err("Session name cannot be empty.".into());
        }
        if self.sessions.len() >= MAX_SESSIONS {
            return Err(format!("Maximum of {MAX_SESSIONS} sessions reached."));
        }
        let id = self.unique_id_for(&trimmed);
        self.sessions.push(WorkspaceSession::new(id.clone(), trimmed));
        self.active_session_id = id.clone();
        Ok(id)
    }

    pub fn duplicate_active_session(&mut self, name: impl Into<String>) -> Result<String, String> {
        let trimmed = name.into().trim().to_string();
        if trimmed.is_empty() {
            return Err("Session name cannot be empty.".into());
        }
        if self.sessions.len() >= MAX_SESSIONS {
            return Err(format!("Maximum of {MAX_SESSIONS} sessions reached."));
        }
        let mut cloned = self.active_session().clone();
        let id = self.unique_id_for(&trimmed);
        cloned.id = id.clone();
        cloned.name = trimmed;
        cloned.updated_at_secs = now_secs();
        self.sessions.push(cloned);
        self.active_session_id = id.clone();
        Ok(id)
    }

    pub fn rename_session(&mut self, id: &str, new_name: impl Into<String>) -> Result<(), String> {
        let trimmed = new_name.into().trim().to_string();
        if trimmed.is_empty() {
            return Err("Session name cannot be empty.".into());
        }
        let Some(sess) = self.sessions.iter_mut().find(|s| s.id == id) else {
            return Err("Session not found.".into());
        };
        sess.name = trimmed;
        sess.updated_at_secs = now_secs();
        Ok(())
    }

    pub fn delete_session(&mut self, id: &str) -> Result<(), String> {
        if self.sessions.len() <= 1 {
            return Err("Cannot delete the last remaining session.".into());
        }
        let Some(pos) = self.sessions.iter().position(|s| s.id == id) else {
            return Err("Session not found.".into());
        };
        self.sessions.remove(pos);
        if self.active_session_id == id {
            self.active_session_id = self.sessions[0].id.clone();
        }
        Ok(())
    }

    fn unique_id_for(&self, name: &str) -> String {
        let slug: String = name
            .chars()
            .filter_map(|c| {
                if c.is_ascii_alphanumeric() {
                    Some(c.to_ascii_lowercase())
                } else if c.is_whitespace() || c == '-' || c == '_' {
                    Some('-')
                } else {
                    None
                }
            })
            .take(24)
            .collect();
        let base = if slug.trim_matches('-').is_empty() {
            "session".to_string()
        } else {
            slug.trim_matches('-').to_string()
        };
        let stamp = now_secs() % 100_000;
        let mut candidate = format!("sess-{base}-{stamp}");
        let mut suffix = 1u32;
        while self.sessions.iter().any(|s| s.id == candidate) {
            candidate = format!("sess-{base}-{stamp}-{suffix}");
            suffix += 1;
        }
        candidate
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_enables_auto_check_updates() {
        let store = SessionsStore::default();
        assert!(store.auto_check_updates);
    }

    #[test]
    fn legacy_sessions_json_defaults_auto_check_updates_to_true() {
        let legacy_json = r#"{
            "version": 1,
            "active_session_id": "default",
            "dark_theme": true,
            "show_system_schemas": false,
            "sessions": [
                {
                    "id": "default",
                    "name": "Default",
                    "active_server_id": null,
                    "active_tab_idx": 0,
                    "next_tab_id": 2,
                    "tabs": [],
                    "connected_servers": [],
                    "updated_at_secs": 1700000000
                }
            ]
        }"#;

        let store: SessionsStore =
            serde_json::from_str(legacy_json).expect("legacy sessions.json should deserialize cleanly");
        assert!(store.auto_check_updates);
        assert!(store.dark_theme);
        assert_eq!(store.active_session_id, "default");
    }

    #[test]
    fn roundtrips_disabled_auto_check_updates() {
        let store = SessionsStore {
            auto_check_updates: false,
            ..SessionsStore::default()
        };
        let json = serde_json::to_string(&store).expect("serialize sessions store");
        let parsed: SessionsStore = serde_json::from_str(&json).expect("deserialize sessions store");
        assert!(!parsed.auto_check_updates);
    }
}

use cottondata_core::{
    ConnectionEndpoint, DatabaseEngine, EnvironmentTag, ServerProfile, ServerSlotId, SshTunnelConfig, TlsMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ServerFormTab {
    General,
    Security,
    Ssh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConnectionTestState {
    Idle,
    Testing(u64),
    Success { summary: String, elapsed_ms: u64 },
    Failed(String),
}

pub(super) struct ServerModalState {
    pub(super) editing_slot: Option<ServerSlotId>,
    pub(super) engine: DatabaseEngine,
    pub(super) name: String,
    pub(super) use_unix_socket: bool,
    pub(super) host: String,
    pub(super) port: String,
    pub(super) socket_path: String,
    pub(super) database_file: String,
    pub(super) username: String,
    pub(super) password: String,
    pub(super) save_in_keychain: bool,
    pub(super) default_schema: String,
    pub(super) tls: TlsMode,
    pub(super) ca_cert_path: String,
    pub(super) ssh_enabled: bool,
    pub(super) ssh_host: String,
    pub(super) ssh_port: String,
    pub(super) ssh_user: String,
    pub(super) ssh_key: String,
    pub(super) environment: EnvironmentTag,
    pub(super) read_only: bool,
    pub(super) page_size: u32,
    pub(super) error_msg: Option<String>,
    pub(super) active_tab: ServerFormTab,
    pub(super) show_password: bool,
    pub(super) show_uri_bar: bool,
    pub(super) uri_input: String,
    pub(super) test_state: ConnectionTestState,
}

impl ServerModalState {
    pub(super) fn new_empty() -> Self {
        Self {
            editing_slot: None,
            engine: DatabaseEngine::MySql,
            name: String::new(),
            use_unix_socket: false,
            host: "127.0.0.1".into(),
            port: "3306".into(),
            socket_path: default_socket_path(DatabaseEngine::MySql).into(),
            database_file: String::new(),
            username: "root".into(),
            password: String::new(),
            save_in_keychain: cfg!(target_os = "macos"),
            default_schema: String::new(),
            tls: TlsMode::Preferred,
            ca_cert_path: String::new(),
            ssh_enabled: false,
            ssh_host: String::new(),
            ssh_port: "22".into(),
            ssh_user: String::new(),
            ssh_key: String::new(),
            environment: EnvironmentTag::Local,
            read_only: false,
            page_size: 500,
            error_msg: None,
            active_tab: ServerFormTab::General,
            show_password: false,
            show_uri_bar: false,
            uri_input: String::new(),
            test_state: ConnectionTestState::Idle,
        }
    }

    pub(super) fn from_existing(slot: ServerSlotId, p: &ServerProfile, pw: Option<String>) -> Self {
        Self::from_profile(Some(slot), p, pw, p.name.clone())
    }

    pub(super) fn from_duplicate(p: &ServerProfile, pw: Option<String>) -> Self {
        Self::from_profile(None, p, pw, format!("{} (Copy)", p.name))
    }

    fn from_profile(editing_slot: Option<ServerSlotId>, p: &ServerProfile, pw: Option<String>, name: String) -> Self {
        let default_port = p.engine.default_port().to_string();
        let default_socket = default_socket_path(p.engine).to_string();
        let (use_unix_socket, host, port, socket_path, database_file) = match &p.endpoint {
            ConnectionEndpoint::Tcp { host, port } => (false, host.clone(), port.to_string(), default_socket, String::new()),
            ConnectionEndpoint::UnixSocket { path } => (true, "127.0.0.1".into(), default_port, path.clone(), String::new()),
            ConnectionEndpoint::File { path } => (false, "127.0.0.1".into(), default_port, default_socket, path.clone()),
        };
        Self {
            editing_slot,
            engine: p.engine,
            name,
            use_unix_socket,
            host,
            port,
            socket_path,
            database_file,
            username: p.username.clone(),
            password: pw.unwrap_or_default(),
            save_in_keychain: cfg!(target_os = "macos"),
            default_schema: p.default_schema.clone().unwrap_or_default(),
            tls: p.tls,
            ca_cert_path: p.ca_cert_path.clone().unwrap_or_default(),
            ssh_enabled: p.ssh.enabled,
            ssh_host: p.ssh.host.clone(),
            ssh_port: p.ssh.port.max(22).to_string(),
            ssh_user: p.ssh.username.clone(),
            ssh_key: p.ssh.identity_file.clone().unwrap_or_default(),
            environment: p.environment,
            read_only: p.read_only,
            page_size: p.page_size,
            error_msg: None,
            active_tab: ServerFormTab::General,
            show_password: false,
            show_uri_bar: false,
            uri_input: String::new(),
            test_state: ConnectionTestState::Idle,
        }
    }

    pub(super) fn switch_engine(&mut self, engine: DatabaseEngine) {
        let is_default_port = self.port.trim() == self.engine.default_port().to_string();
        if is_default_port || self.port.trim().is_empty() {
            self.port = engine.default_port().to_string();
        }
        if !engine.supports_unix_socket() {
            self.use_unix_socket = false;
        }
        if self.socket_path.trim() == default_socket_path(self.engine) || self.socket_path.trim().is_empty() {
            self.socket_path = default_socket_path(engine).into();
        }
        if self.username.trim() == self.engine.default_username() {
            self.username = engine.default_username().into();
        }
        self.engine = engine;
    }

    fn apply_sqlite_location(&mut self, location: &str) -> Result<(), String> {
        let path = percent_decode(location.split('?').next().unwrap_or(location)).trim().to_string();
        if path.is_empty() {
            return Err("SQLite URI is missing the database file path".to_string());
        }
        self.engine = DatabaseEngine::Sqlite;
        self.use_unix_socket = false;
        self.ssh_enabled = false;
        self.database_file = path;
        if self.name.trim().is_empty() {
            let file_name = std::path::Path::new(&self.database_file)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.database_file.clone());
            self.name = format!("{file_name} (SQLite)");
        }
        self.test_state = ConnectionTestState::Idle;
        self.error_msg = None;
        Ok(())
    }

    pub(super) fn apply_connection_uri(&mut self, raw: &str) -> Result<(), String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("Connection URI cannot be empty".to_string());
        }

        if let Some(location) = sqlite_location(trimmed) {
            return self.apply_sqlite_location(location);
        }

        let (scheme, rest) = trimmed.split_once("://").ok_or_else(|| {
            "Connection URI must include a scheme (e.g. mysql://, postgres://, mssql:// or sqlite:)".to_string()
        })?;

        let mut engine = match scheme.to_ascii_lowercase().as_str() {
            "mysql" | "mariadb" => DatabaseEngine::MySql,
            "mssql" | "sqlserver" => DatabaseEngine::SqlServer,
            "postgres" | "postgresql" | "pg" => DatabaseEngine::PostgreSql,
            "cockroachdb" | "cockroach" | "crdb" => DatabaseEngine::CockroachDb,
            _ => {
                return Err(format!(
                    "Unsupported URI scheme '{scheme}'. Expected mysql://, mariadb://, postgres://, postgresql://, cockroachdb://, mssql://, sqlserver:// or sqlite:"
                ));
            }
        };

        let (before_query, raw_query) = match rest.split_once('?') {
            Some((before, q)) => (before, Some(q.split('#').next().unwrap_or(q))),
            None => (rest, None),
        };

        let (authority, raw_path) = match before_query.split_once('/') {
            Some((auth, path)) => (auth, Some(path.split('#').next().unwrap_or(path))),
            None => (before_query, None),
        };

        let (userinfo, host_port) = match authority.rsplit_once('@') {
            Some((info, hp)) => (Some(info), hp),
            None => (None, authority),
        };

        let (parsed_user, parsed_pass) = match userinfo {
            Some(info) if !info.is_empty() => match info.split_once(':') {
                Some((u, p)) => (Some(percent_decode(u)), Some(percent_decode(p))),
                None => (Some(percent_decode(info)), None),
            },
            _ => (None, None),
        };

        let (host, port_str) = if let Some(after_open) = host_port.strip_prefix('[') {
            let (ipv6, after_close) = after_open
                .split_once(']')
                .ok_or_else(|| "Invalid IPv6 host: missing closing ']'".to_string())?;
            if ipv6.trim().is_empty() {
                return Err("Connection URI is missing a host".to_string());
            }
            let port_part = if after_close.is_empty() {
                None
            } else if let Some(p) = after_close.strip_prefix(':') {
                Some(p)
            } else {
                return Err(format!("Invalid host/port syntax: '{host_port}'"));
            };
            (ipv6.trim().to_string(), port_part)
        } else if let Some((h, p)) = host_port.rsplit_once(':') {
            if h.contains(':') {
                return Err("IPv6 addresses must be enclosed in brackets, e.g. [::1]:3306".to_string());
            }
            (percent_decode(h).trim().to_string(), Some(p))
        } else {
            (percent_decode(host_port).trim().to_string(), None)
        };

        if host.is_empty() {
            return Err("Connection URI is missing a host".to_string());
        }
        let is_cockroach_endpoint =
            port_str == Some("26257") || host.to_ascii_lowercase().ends_with(".cockroachlabs.cloud");
        if engine == DatabaseEngine::PostgreSql && is_cockroach_endpoint {
            engine = DatabaseEngine::CockroachDb;
        }

        let port = match port_str {
            Some(p) if !p.is_empty() => {
                let parsed = p
                    .parse::<u16>()
                    .map_err(|_| format!("Invalid port '{p}': must be a number between 1 and 65535"))?;
                if parsed == 0 {
                    return Err("Invalid port '0': must be a number between 1 and 65535".to_string());
                }
                parsed
            }
            Some(_) => return Err("Connection URI has an empty port after ':'".to_string()),
            None => engine.default_port(),
        };

        let mut parsed_schema = raw_path
            .map(|p| p.trim_end_matches('/'))
            .filter(|p| !p.is_empty())
            .map(percent_decode)
            .filter(|s| !s.trim().is_empty());

        let mut parsed_tls: Option<TlsMode> = None;
        let mut parsed_ca_path: Option<String> = None;

        if let Some(query) = raw_query {
            for pair in query.split('&') {
                if pair.is_empty() {
                    continue;
                }
                let Some((k, v)) = pair.split_once('=') else {
                    continue;
                };
                let key = percent_decode(k).trim().to_ascii_lowercase();
                let val = percent_decode(v).trim().to_string();
                match key.as_str() {
                    "ssl-mode" | "sslmode" | "encrypt" => {
                        if let Some(mode) = parse_tls_mode(&val) {
                            parsed_tls = Some(mode);
                        }
                    }
                    "database" | "dbname" => {
                        if parsed_schema.is_none() && !val.is_empty() {
                            parsed_schema = Some(val);
                        }
                    }
                    "sslrootcert" | "ssl-ca" | "sslca" => {
                        if !val.is_empty() {
                            parsed_ca_path = Some(val);
                        }
                    }
                    _ => {}
                }
            }
        }

        let previous_engine = self.engine;
        if self.socket_path.trim() == default_socket_path(previous_engine) {
            self.socket_path = default_socket_path(engine).into();
        }
        self.engine = engine;
        self.use_unix_socket = false;
        self.host = host;
        self.port = port.to_string();

        if let Some(user) = parsed_user {
            if !user.is_empty() {
                self.username = user;
            }
        } else if self.username.trim().is_empty() || self.username.trim() == previous_engine.default_username() {
            self.username = engine.default_username().into();
        }

        if let Some(pass) = parsed_pass {
            self.password = pass;
        }

        if let Some(schema) = parsed_schema {
            self.default_schema = schema;
        }

        if let Some(tls) = parsed_tls {
            self.tls = tls;
        }

        if let Some(ca_path) = parsed_ca_path {
            self.ca_cert_path = ca_path;
        }

        if self.name.trim().is_empty() {
            let schema_or_host = if self.default_schema.trim().is_empty() {
                self.host.as_str()
            } else {
                self.default_schema.as_str()
            };
            self.name = format!("{} ({})", schema_or_host, self.engine.label());
        }

        self.test_state = ConnectionTestState::Idle;
        self.error_msg = None;
        Ok(())
    }

    pub(super) fn build_profile(&self, existing_id: Option<String>) -> Result<ServerProfile, String> {
        let endpoint = if self.engine.is_file_based() {
            ConnectionEndpoint::File {
                path: self.database_file.trim().to_string(),
            }
        } else if self.use_unix_socket && self.engine.supports_unix_socket() {
            ConnectionEndpoint::UnixSocket {
                path: self.socket_path.trim().to_string(),
            }
        } else {
            let port = self
                .port
                .trim()
                .parse::<u16>()
                .map_err(|_| "Port must be a number between 1 and 65535".to_string())?;
            ConnectionEndpoint::Tcp {
                host: self.host.trim().to_string(),
                port,
            }
        };
        let ssh_port = self.ssh_port.trim().parse::<u16>().unwrap_or(22);
        let id = existing_id.unwrap_or_else(|| {
            let slug: String = self
                .name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() {
                        c.to_ascii_lowercase()
                    } else {
                        '-'
                    }
                })
                .collect();
            format!(
                "srv-{slug}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() % 100_000)
                    .unwrap_or(1)
            )
        });

        let profile = ServerProfile {
            id,
            engine: self.engine,
            name: self.name.trim().to_string(),
            endpoint,
            username: self.username.trim().to_string(),
            default_schema: {
                let s = self.default_schema.trim();
                if s.is_empty() {
                    None
                } else {
                    Some(s.to_string())
                }
            },
            tls: self.tls,
            ca_cert_path: {
                let s = self.ca_cert_path.trim();
                if s.is_empty() {
                    None
                } else {
                    Some(s.to_string())
                }
            },
            ssh: SshTunnelConfig {
                enabled: self.ssh_enabled && !self.engine.is_file_based(),
                host: self.ssh_host.trim().to_string(),
                port: ssh_port,
                username: self.ssh_user.trim().to_string(),
                identity_file: {
                    let s = self.ssh_key.trim();
                    if s.is_empty() {
                        None
                    } else {
                        Some(s.to_string())
                    }
                },
                local_bind_port: None,
            },
            environment: self.environment,
            read_only: self.read_only,
            page_size: self.page_size,
            connect_timeout_secs: 10,
            idle_disconnect_mins: 15,
            show_system_schemas: false,
        };
        profile.validate().map_err(|e| e.to_string())?;
        Ok(profile)
    }
}

fn default_socket_path(engine: DatabaseEngine) -> &'static str {
    match engine {
        DatabaseEngine::PostgreSql => "/tmp/.s.PGSQL.5432",
        _ => "/tmp/mysql.sock",
    }
}

fn sqlite_location(uri: &str) -> Option<&str> {
    let lower = uri.to_ascii_lowercase();
    for prefix in ["sqlite3://", "sqlite://", "sqlite3:", "sqlite:", "file://", "file:"] {
        if lower.starts_with(prefix) {
            return Some(&uri[prefix.len()..]);
        }
    }
    let is_bare_database_file = !uri.contains("://")
        && [".db", ".sqlite", ".sqlite3", ".db3"]
            .iter()
            .any(|extension| lower.ends_with(extension));
    is_bare_database_file.then_some(uri)
}

fn parse_tls_mode(value: &str) -> Option<TlsMode> {
    match value.to_ascii_lowercase().as_str() {
        "disabled" | "disable" | "false" => Some(TlsMode::Disabled),
        "preferred" | "prefer" | "allow" => Some(TlsMode::Preferred),
        "required" | "require" | "true" => Some(TlsMode::Required),
        "verify_ca" | "verify-ca" => Some(TlsMode::VerifyCa),
        "verify_identity" | "verify-identity" | "verify-full" | "verify_full" => Some(TlsMode::VerifyIdentity),
        _ => None,
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2])) {
                out.push((hi << 4) | lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(&e.into_bytes()).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mysql_uri_with_percent_encoded_password_port_schema_and_tls() {
        let mut form = ServerModalState::new_empty();
        form.use_unix_socket = true;
        form.error_msg = Some("previous error".into());
        form.test_state = ConnectionTestState::Failed("old failure".into());

        form.apply_connection_uri("mysql://app_user:p%40ss%3A123@db.prod.internal:3307/orders%20db?ssl-mode=required")
            .expect("valid mysql URI");

        assert_eq!(form.engine, DatabaseEngine::MySql);
        assert!(!form.use_unix_socket);
        assert_eq!(form.username, "app_user");
        assert_eq!(form.password, "p@ss:123");
        assert_eq!(form.host, "db.prod.internal");
        assert_eq!(form.port, "3307");
        assert_eq!(form.default_schema, "orders db");
        assert_eq!(form.tls, TlsMode::Required);
        assert_eq!(form.name, "orders db (MySQL)");
        assert_eq!(form.test_state, ConnectionTestState::Idle);
        assert_eq!(form.error_msg, None);
    }

    #[test]
    fn parses_sqlserver_uri_with_default_port_and_database() {
        let mut form = ServerModalState::new_empty();
        form.apply_connection_uri("sqlserver://sa:Str0ng%2FPass%251@mssql.local/reporting?encrypt=true")
            .expect("valid sqlserver URI");

        assert_eq!(form.engine, DatabaseEngine::SqlServer);
        assert_eq!(form.username, "sa");
        assert_eq!(form.password, "Str0ng/Pass%1");
        assert_eq!(form.host, "mssql.local");
        assert_eq!(form.port, "1433");
        assert_eq!(form.default_schema, "reporting");
        assert_eq!(form.tls, TlsMode::Required);
        assert_eq!(form.name, "reporting (SQL Server)");

        let mut query_db_form = ServerModalState::new_empty();
        query_db_form
            .apply_connection_uri("mssql://admin:pw@10.0.0.9?database=analytics&encrypt=false")
            .expect("valid mssql URI with database query param");
        assert_eq!(query_db_form.engine, DatabaseEngine::SqlServer);
        assert_eq!(query_db_form.port, "1433");
        assert_eq!(query_db_form.default_schema, "analytics");
        assert_eq!(query_db_form.tls, TlsMode::Disabled);
    }

    #[test]
    fn parses_mariadb_uri_without_password_and_default_port() {
        let mut form = ServerModalState::new_empty();
        form.password = "existing-secret".into();
        form.name = "Existing Name".into();

        form.apply_connection_uri("mariadb://readonly@db.staging.local/catalog?sslmode=verify-identity")
            .expect("valid mariadb URI");

        assert_eq!(form.engine, DatabaseEngine::MySql);
        assert_eq!(form.username, "readonly");
        assert_eq!(form.password, "existing-secret");
        assert_eq!(form.host, "db.staging.local");
        assert_eq!(form.port, "3306");
        assert_eq!(form.default_schema, "catalog");
        assert_eq!(form.tls, TlsMode::VerifyIdentity);
        assert_eq!(form.name, "Existing Name");
    }

    #[test]
    fn parses_ipv6_bracketed_host_with_and_without_port() {
        let mut form = ServerModalState::new_empty();
        form.apply_connection_uri("mysql://root:pw@[::1]:3308/localdb?ssl-mode=verify_ca")
            .expect("valid ipv6 URI with port");
        assert_eq!(form.host, "::1");
        assert_eq!(form.port, "3308");
        assert_eq!(form.default_schema, "localdb");
        assert_eq!(form.tls, TlsMode::VerifyCa);

        let mut default_port_form = ServerModalState::new_empty();
        default_port_form
            .apply_connection_uri("mysql://root@[::1]")
            .expect("valid ipv6 URI with default port");
        assert_eq!(default_port_form.host, "::1");
        assert_eq!(default_port_form.port, "3306");
        assert_eq!(default_port_form.name, "::1 (MySQL)");
    }

    #[test]
    fn rejects_invalid_scheme_missing_host_or_bad_port() {
        let mut form = ServerModalState::new_empty();
        assert!(form.apply_connection_uri("").is_err());
        assert!(form.apply_connection_uri("localhost:3306").is_err());
        assert!(form.apply_connection_uri("redis://user:pass@localhost:6379/0").is_err());
        assert!(form.apply_connection_uri("sqlite:").is_err());
        assert!(form.apply_connection_uri("mysql:///orders").is_err());
        assert!(form.apply_connection_uri("mysql://user:pass@/orders").is_err());
        assert!(form.apply_connection_uri("mysql://user:pass@[]:3306/orders").is_err());
        assert!(form.apply_connection_uri("mysql://user:pass@[::1/orders").is_err());
        assert!(form.apply_connection_uri("mysql://user:pass@localhost:abc/orders").is_err());
        assert!(form.apply_connection_uri("mysql://user:pass@localhost:0/orders").is_err());
        assert!(form.apply_connection_uri("mysql://user:pass@localhost:/orders").is_err());
    }

    #[test]
    fn from_duplicate_clears_editing_slot_and_appends_copy_suffix() {
        let mut profile = ServerProfile::new_local("Production DB", "10.20.30.40", 3307, "deploy");
        profile.engine = DatabaseEngine::SqlServer;
        profile.default_schema = Some("core".into());
        profile.tls = TlsMode::VerifyIdentity;
        profile.ca_cert_path = Some("/etc/ssl/ca.pem".into());
        profile.ssh = SshTunnelConfig {
            enabled: true,
            host: "bastion.example.com".into(),
            port: 2222,
            username: "ops".into(),
            identity_file: Some("/Users/dev/.ssh/id_ed25519".into()),
            local_bind_port: None,
        };
        profile.environment = EnvironmentTag::Production;
        profile.read_only = true;
        profile.page_size = 1000;

        let dup = ServerModalState::from_duplicate(&profile, Some("s3cr3t".into()));

        assert_eq!(dup.editing_slot, None);
        assert_eq!(dup.name, "Production DB (Copy)");
        assert_eq!(dup.engine, DatabaseEngine::SqlServer);
        assert!(!dup.use_unix_socket);
        assert_eq!(dup.host, "10.20.30.40");
        assert_eq!(dup.port, "3307");
        assert_eq!(dup.username, "deploy");
        assert_eq!(dup.password, "s3cr3t");
        assert_eq!(dup.default_schema, "core");
        assert_eq!(dup.tls, TlsMode::VerifyIdentity);
        assert_eq!(dup.ca_cert_path, "/etc/ssl/ca.pem");
        assert!(dup.ssh_enabled);
        assert_eq!(dup.ssh_host, "bastion.example.com");
        assert_eq!(dup.ssh_port, "2222");
        assert_eq!(dup.ssh_user, "ops");
        assert_eq!(dup.ssh_key, "/Users/dev/.ssh/id_ed25519");
        assert_eq!(dup.environment, EnvironmentTag::Production);
        assert!(dup.read_only);
        assert_eq!(dup.page_size, 1000);
        assert_eq!(dup.active_tab, ServerFormTab::General);
        assert!(!dup.show_password);
        assert!(!dup.show_uri_bar);
        assert!(dup.uri_input.is_empty());
        assert_eq!(dup.test_state, ConnectionTestState::Idle);
    }

    #[test]
    fn parses_postgres_uri_with_libpq_ssl_options() {
        let mut form = ServerModalState::new_empty();
        form.apply_connection_uri(
            "postgresql://app:s%3Acret@db.internal/billing?sslmode=verify-full&sslrootcert=%2Fetc%2Fca.pem",
        )
        .expect("valid postgres URI");
        assert_eq!(form.engine, DatabaseEngine::PostgreSql);
        assert_eq!(form.port, "5432");
        assert_eq!(form.username, "app");
        assert_eq!(form.password, "s:cret");
        assert_eq!(form.default_schema, "billing");
        assert_eq!(form.tls, TlsMode::VerifyIdentity);
        assert_eq!(form.ca_cert_path, "/etc/ca.pem");
        assert_eq!(form.name, "billing (PostgreSQL)");

        let mut prefer = ServerModalState::new_empty();
        prefer
            .apply_connection_uri("postgres://localhost?sslmode=disable")
            .expect("bare postgres URI");
        assert_eq!(prefer.tls, TlsMode::Disabled);
        assert_eq!(prefer.username, "postgres");
    }

    #[test]
    fn detects_cockroach_from_scheme_port_or_cloud_host() {
        let mut by_scheme = ServerModalState::new_empty();
        by_scheme
            .apply_connection_uri("cockroachdb://root@localhost/defaultdb")
            .expect("cockroach scheme");
        assert_eq!(by_scheme.engine, DatabaseEngine::CockroachDb);
        assert_eq!(by_scheme.port, "26257");

        let mut by_port = ServerModalState::new_empty();
        by_port
            .apply_connection_uri("postgresql://root@127.0.0.1:26257/bank")
            .expect("cockroach port");
        assert_eq!(by_port.engine, DatabaseEngine::CockroachDb);

        let mut by_host = ServerModalState::new_empty();
        by_host
            .apply_connection_uri("postgresql://u:p@free-tier.gcp-us-central1.cockroachlabs.cloud/defaultdb?sslmode=verify-full")
            .expect("cockroach cloud");
        assert_eq!(by_host.engine, DatabaseEngine::CockroachDb);
        assert_eq!(by_host.port, "26257");
    }

    #[test]
    fn sqlite_uris_and_bare_database_paths_build_file_profiles() {
        let mut form = ServerModalState::new_empty();
        form.apply_connection_uri("sqlite:///Users/dev/app%20data/shop.sqlite3")
            .expect("sqlite uri");
        assert_eq!(form.engine, DatabaseEngine::Sqlite);
        assert_eq!(form.database_file, "/Users/dev/app data/shop.sqlite3");
        assert_eq!(form.name, "shop.sqlite3 (SQLite)");
        let profile = form.build_profile(None).expect("sqlite profile");
        assert_eq!(
            profile.endpoint,
            ConnectionEndpoint::File {
                path: "/Users/dev/app data/shop.sqlite3".into()
            }
        );
        assert!(!profile.ssh.enabled);

        let mut bare = ServerModalState::new_empty();
        bare.apply_connection_uri("/tmp/cache.db").expect("bare path");
        assert_eq!(bare.engine, DatabaseEngine::Sqlite);
        assert_eq!(bare.database_file, "/tmp/cache.db");
    }

    #[test]
    fn switching_engine_updates_default_port_username_and_socket() {
        let mut form = ServerModalState::new_empty();
        form.switch_engine(DatabaseEngine::PostgreSql);
        assert_eq!(form.port, "5432");
        assert_eq!(form.username, "postgres");
        assert_eq!(form.socket_path, "/tmp/.s.PGSQL.5432");
        form.switch_engine(DatabaseEngine::CockroachDb);
        assert_eq!(form.port, "26257");
        assert_eq!(form.username, "root");
        assert!(!form.use_unix_socket);
    }
}

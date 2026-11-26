use cottondata_core::{ConnectionEndpoint, EnvironmentTag, ServerProfile, ServerSlotId, SshTunnelConfig, TlsMode};

pub(super) struct ServerModalState {
    pub(super) editing_slot: Option<ServerSlotId>,
    pub(super) name: String,
    pub(super) use_unix_socket: bool,
    pub(super) host: String,
    pub(super) port: String,
    pub(super) socket_path: String,
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
}

impl ServerModalState {
    pub(super) fn new_empty() -> Self {
        Self {
            editing_slot: None,
            name: String::new(),
            use_unix_socket: false,
            host: "127.0.0.1".into(),
            port: "3306".into(),
            socket_path: "/tmp/mysql.sock".into(),
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
        }
    }

    pub(super) fn from_existing(slot: ServerSlotId, p: &ServerProfile, pw: Option<String>) -> Self {
        let (use_unix_socket, host, port, socket_path) = match &p.endpoint {
            ConnectionEndpoint::Tcp { host, port } => (false, host.clone(), port.to_string(), "/tmp/mysql.sock".into()),
            ConnectionEndpoint::UnixSocket { path } => (true, "127.0.0.1".into(), "3306".into(), path.clone()),
        };
        Self {
            editing_slot: Some(slot),
            name: p.name.clone(),
            use_unix_socket,
            host,
            port,
            socket_path,
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
        }
    }

    pub(super) fn build_profile(&self, existing_id: Option<String>) -> Result<ServerProfile, String> {
        let endpoint = if self.use_unix_socket {
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
                enabled: self.ssh_enabled,
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

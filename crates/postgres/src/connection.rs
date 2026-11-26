use crate::cancel::{self, CancelHandle};
use crate::config::plan_connection;
use crate::errors::postgres_error;
use crate::introspection::Flavor;
use crate::metadata::MetadataCache;
use crate::session::PostgresSession;
use crate::tls::{build_tls_connector, tls_policy};
use cottondata_core::{ServerProfile, ServerRuntimeInfo};
use cottondata_driver::DbError;
use std::time::Duration;
use tokio_postgres::{Client, SimpleQueryMessage};

pub(crate) struct ServerIdentity {
    pub(crate) version_text: String,
    pub(crate) current_schema: Option<String>,
    pub(crate) time_zone: String,
    pub(crate) backend_pid: Option<u32>,
    pub(crate) cockroach_session_id: Option<String>,
}

pub(crate) async fn load_identity(client: &Client) -> Result<ServerIdentity, DbError> {
    let messages = client
        .simple_query("SELECT version(), current_schema(), current_setting('TimeZone'), pg_backend_pid()::text")
        .await
        .map_err(postgres_error)?;
    let row = messages
        .iter()
        .find_map(|message| match message {
            SimpleQueryMessage::Row(row) => Some(row),
            _ => None,
        })
        .ok_or_else(|| DbError::message("The server did not return version information"))?;
    let version_text = row.get(0).unwrap_or_default().to_string();
    let cockroach_session_id = if version_text.contains("CockroachDB") {
        client
            .simple_query("SHOW session_id")
            .await
            .ok()
            .and_then(|messages| first_value(&messages))
    } else {
        None
    };
    Ok(ServerIdentity {
        current_schema: row.get(1).map(str::to_string),
        time_zone: row.get(2).unwrap_or_default().to_string(),
        backend_pid: row.get(3).and_then(|pid| pid.parse().ok()),
        version_text,
        cockroach_session_id,
    })
}

pub(crate) fn first_value(messages: &[SimpleQueryMessage]) -> Option<String> {
    messages.iter().find_map(|message| match message {
        SimpleQueryMessage::Row(row) => row.get(0).map(str::to_string),
        _ => None,
    })
}

pub(crate) fn parse_server_version(version_text: &str) -> (String, (u16, u16, u16)) {
    let display = if let Some(position) = version_text.find("CockroachDB") {
        version_text[position..]
            .split_whitespace()
            .find(|word| word.starts_with('v') && word[1..].starts_with(|c: char| c.is_ascii_digit()))
            .map(str::to_string)
            .unwrap_or_else(|| "unknown".to_string())
    } else {
        version_text
            .split_whitespace()
            .nth(1)
            .filter(|_| version_text.starts_with("PostgreSQL"))
            .map(str::to_string)
            .unwrap_or_else(|| version_text.split_whitespace().next().unwrap_or_default().to_string())
    };
    let numeric = display
        .trim_start_matches('v')
        .split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .next()
        .unwrap_or_default();
    (display.clone(), ServerRuntimeInfo::parse_version_tuple(numeric))
}

impl PostgresSession {
    pub fn connect(profile: &ServerProfile, password: Option<&str>) -> Result<Self, DbError> {
        let timeout = Duration::from_secs(u64::from(profile.connect_timeout_secs.max(3)));
        let policy = tls_policy(profile.tls);
        let tls = build_tls_connector(policy.check, profile.ca_cert_path.as_deref())?;
        let plan = plan_connection(profile, password, policy.ssl_mode, timeout)?;
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| DbError::message(format!("Could not start the PostgreSQL runtime: {err}")))?;
        let (client, connection) = executor
            .block_on(async { tokio::time::timeout(timeout, plan.config.connect(tls.clone())).await })
            .map_err(|_| {
                DbError::message(format!(
                    "Timed out connecting to {} at {}",
                    profile.engine.label(),
                    profile.endpoint.display_address()
                ))
            })?
            .map_err(postgres_error)?;
        executor.spawn(async move {
            let _ = connection.await;
        });

        let identity = executor.block_on(load_identity(&client))?;
        let flavor = if identity.version_text.contains("CockroachDB") {
            Flavor::Cockroach
        } else {
            Flavor::Postgres
        };
        let (version, (version_major, version_minor, version_patch)) = parse_server_version(&identity.version_text);
        let is_cursor_supported = flavor == Flavor::Postgres;
        let connection_id = match (flavor, identity.backend_pid) {
            (Flavor::Postgres, Some(pid)) => pid,
            _ => cancel::next_synthetic_connection_id(),
        };
        if profile.read_only {
            executor
                .block_on(client.batch_execute("SET default_transaction_read_only = on"))
                .map_err(postgres_error)?;
        }
        let overflow_canceller = identity
            .cockroach_session_id
            .clone()
            .map(|session_id| cancel::cockroach_overflow_canceller(profile, password, session_id));
        cancel::register(
            &profile.id,
            connection_id,
            CancelHandle {
                token: client.cancel_token(),
                tls,
                cockroach_session_id: identity.cockroach_session_id,
            },
        );

        Ok(Self {
            executor,
            client,
            tunnel: plan.tunnel,
            runtime: ServerRuntimeInfo {
                version,
                version_major,
                version_minor,
                version_patch,
                lower_case_table_names: 0,
                sql_mode: String::new(),
                time_zone: identity.time_zone,
                current_schema: identity.current_schema,
                connection_id,
                autocommit: true,
                in_transaction: false,
            },
            flavor,
            profile_id: profile.id.clone(),
            read_only: profile.read_only,
            is_cursor_supported,
            metadata: MetadataCache::default(),
            cockroach_query_ids: Vec::new(),
            overflow_canceller,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_postgres_and_cockroach_version_strings() {
        let (display, tuple) = parse_server_version(
            "PostgreSQL 16.2 (Debian 16.2-1.pgdg120+2) on aarch64-unknown-linux-gnu, compiled by gcc",
        );
        assert_eq!(display, "16.2");
        assert_eq!(tuple, (16, 2, 0));

        let (display, tuple) =
            parse_server_version("CockroachDB CCL v23.2.4 (aarch64-unknown-linux-gnu, built 2024/04/03, go1.21.5)");
        assert_eq!(display, "v23.2.4");
        assert_eq!(tuple, (23, 2, 4));

        let (_, tuple) = parse_server_version("PostgreSQL 9.6.24 on x86_64-pc-linux-gnu");
        assert_eq!(tuple, (9, 6, 24));
    }
}

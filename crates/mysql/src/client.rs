use crate::tunnel::SshTunnel;
use cottondata_core::{
    ConnectionEndpoint, MySqlDiagnostic, ServerProfile, ServerRuntimeInfo, TlsMode,
};
use cottondata_grid::{CellValue, ColumnMeta, ColumnarPage};
use cottondata_sql::analyze_statement;
use mysql::consts::{ColumnFlags, ColumnType};
use mysql::prelude::Queryable;
use mysql::{Conn, OptsBuilder, SslOpts, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct MySqlError {
    pub diagnostic: MySqlDiagnostic,
}

impl MySqlError {
    pub fn from_mysql(err: mysql::Error) -> Self {
        let code = match &err {
            mysql::Error::MySqlError(me) => Some(me.code),
            _ => None,
        };
        Self {
            diagnostic: MySqlDiagnostic::classify(err.to_string(), code),
        }
    }

    pub fn message(msg: impl Into<String>) -> Self {
        Self {
            diagnostic: MySqlDiagnostic::classify(msg.into(), None),
        }
    }
}

impl std::fmt::Display for MySqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.diagnostic.summary)?;
        if !self.diagnostic.raw_message.is_empty()
            && self.diagnostic.raw_message != self.diagnostic.summary
        {
            write!(f, " [{}]", self.diagnostic.raw_message)?;
        }
        if let Some(hint) = &self.diagnostic.hint {
            write!(f, " — {hint}")?;
        }
        Ok(())
    }
}

impl std::error::Error for MySqlError {}

#[derive(Debug, Clone)]
pub enum QueryOutcome {
    Rows(ColumnarPage),
    Affected {
        affected_rows: u64,
        last_insert_id: Option<u64>,
        warnings: u16,
        elapsed_ms: u64,
        executed_sql: String,
    },
}

#[derive(Debug, Clone)]
pub struct ExplainResult {
    pub is_analyze: bool,
    pub raw_output: String,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone)]
pub struct ProcessEntry {
    pub id: u64,
    pub user: String,
    pub host: String,
    pub db: Option<String>,
    pub command: String,
    pub time_secs: i64,
    pub state: Option<String>,
    pub info: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ServerHealthMetrics {
    pub uptime_secs: u64,
    pub threads_connected: u64,
    pub threads_running: u64,
    pub questions: u64,
    pub innodb_hit_ratio_pct: Option<f64>,
}

pub struct MySqlSession {
    conn: Conn,
    tunnel: Option<SshTunnel>,
    runtime: ServerRuntimeInfo,
    read_only: bool,
}

impl MySqlSession {
    pub fn connect(profile: &ServerProfile, password: Option<&str>) -> Result<Self, MySqlError> {
        let timeout = Duration::from_secs(profile.connect_timeout_secs.max(3) as u64);
        let (tunnel, mut conn) = connect_with_tls_fallback(profile, password, timeout)?;
        let runtime = load_runtime_info(&mut conn)?;
        Ok(Self {
            conn,
            tunnel,
            runtime,
            read_only: profile.read_only,
        })
    }

    pub fn runtime(&self) -> &ServerRuntimeInfo {
        &self.runtime
    }

    pub fn conn_mut(&mut self) -> &mut Conn {
        &mut self.conn
    }

    pub fn ping(&mut self) -> Result<(), MySqlError> {
        if let Some(tunnel) = &mut self.tunnel {
            if !tunnel.is_alive() {
                return Err(MySqlError::message("SSH tunnel process terminated"));
            }
        }
        self.conn.ping().map_err(MySqlError::from_mysql)
    }

    pub fn set_schema(&mut self, schema: &str) -> Result<(), MySqlError> {
        let escaped = schema.replace('`', "``");
        self.conn
            .query_drop(format!("USE `{escaped}`"))
            .map_err(MySqlError::from_mysql)?;
        self.runtime.current_schema = Some(schema.to_string());
        Ok(())
    }

    pub fn set_autocommit(&mut self, enabled: bool) -> Result<(), MySqlError> {
        let val = if enabled { 1 } else { 0 };
        self.conn
            .query_drop(format!("SET autocommit = {val}"))
            .map_err(MySqlError::from_mysql)?;
        self.runtime.autocommit = enabled;
        if enabled {
            self.runtime.in_transaction = false;
        }
        Ok(())
    }

    pub fn begin_transaction(&mut self) -> Result<(), MySqlError> {
        self.conn
            .query_drop("START TRANSACTION")
            .map_err(MySqlError::from_mysql)?;
        self.runtime.in_transaction = true;
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), MySqlError> {
        self.conn
            .query_drop("COMMIT")
            .map_err(MySqlError::from_mysql)?;
        self.runtime.in_transaction = false;
        Ok(())
    }

    pub fn rollback(&mut self) -> Result<(), MySqlError> {
        self.conn
            .query_drop("ROLLBACK")
            .map_err(MySqlError::from_mysql)?;
        self.runtime.in_transaction = false;
        Ok(())
    }

    pub fn execute_sql(&mut self, sql: &str, max_rows: u32) -> Result<QueryOutcome, MySqlError> {
        let safety = analyze_statement(sql);
        if self.read_only && !safety.allowed_in_read_only() {
            return Err(MySqlError::message(
                "Blocked by server Read-Only protection. Disable Read-Only on this server slot to execute writes.",
            ));
        }

        let start = Instant::now();
        let mut result = self.conn.query_iter(sql).map_err(MySqlError::from_mysql)?;
        let cols_ref = result.columns();
        let cols_slice = cols_ref.as_ref();

        if cols_slice.is_empty() {
            let affected_rows = result.affected_rows();
            let last_insert_id = result.last_insert_id();
            let warnings = result.warnings();
            drop(result);
            self.sync_tx_state_after_stmt(sql);
            return Ok(QueryOutcome::Affected {
                affected_rows,
                last_insert_id,
                warnings,
                elapsed_ms: start.elapsed().as_millis() as u64,
                executed_sql: sql.to_string(),
            });
        }

        let columns: Vec<ColumnMeta> = cols_slice
            .iter()
            .map(|c| {
                let flags = c.flags();
                let col_type = c.column_type();
                ColumnMeta {
                    name: c.name_str().into_owned(),
                    table: {
                        let t = c.org_table_str();
                        if t.is_empty() { None } else { Some(t.into_owned()) }
                    },
                    schema: {
                        let s = c.schema_str();
                        if s.is_empty() { None } else { Some(s.into_owned()) }
                    },
                    type_name: mysql_type_label(col_type, flags),
                    is_primary_key: flags.contains(ColumnFlags::PRI_KEY_FLAG),
                    is_nullable: !flags.contains(ColumnFlags::NOT_NULL_FLAG),
                    is_auto_increment: flags.contains(ColumnFlags::AUTO_INCREMENT_FLAG),
                    is_numeric: is_numeric_mysql_type(col_type),
                }
            })
            .collect();

        let col_types: Vec<(ColumnType, ColumnFlags)> = cols_slice
            .iter()
            .map(|c| (c.column_type(), c.flags()))
            .collect();

        let mut page = ColumnarPage::new(columns, sql.to_string());
        let cap = max_rows.max(1) as usize;
        let mut row_buf = Vec::with_capacity(col_types.len());

        for row_res in result.by_ref() {
            let row = row_res.map_err(MySqlError::from_mysql)?;
            if page.row_count >= cap {
                page.has_more_rows = true;
                break;
            }
            row_buf.clear();
            for (idx, (col_type, flags)) in col_types.iter().enumerate() {
                let val = row.as_ref(idx).cloned().unwrap_or(Value::NULL);
                row_buf.push(format_mysql_value(val, *col_type, *flags));
            }
            page.push_row(&row_buf);
        }

        drop(result);
        page.elapsed_ms = start.elapsed().as_millis() as u64;
        self.sync_tx_state_after_stmt(sql);
        Ok(QueryOutcome::Rows(page))
    }

    pub fn execute_atomic_batch(&mut self, statements: &[String]) -> Result<u64, MySqlError> {
        if self.read_only {
            return Err(MySqlError::message("Cannot submit changes while Read-Only protection is enabled."));
        }
        if statements.is_empty() {
            return Ok(0);
        }

        let start = Instant::now();
        self.conn
            .query_drop("START TRANSACTION")
            .map_err(MySqlError::from_mysql)?;
        for stmt in statements {
            if let Err(err) = self.conn.query_drop(stmt) {
                let _ = self.conn.query_drop("ROLLBACK");
                return Err(MySqlError::from_mysql(err));
            }
        }
        self.conn
            .query_drop("COMMIT")
            .map_err(MySqlError::from_mysql)?;
        self.runtime.in_transaction = false;
        Ok(start.elapsed().as_millis() as u64)
    }

    pub fn explain(&mut self, sql: &str, analyze: bool) -> Result<ExplainResult, MySqlError> {
        if analyze && !self.runtime.supports_explain_analyze() {
            return Err(MySqlError::message(format!(
                "EXPLAIN ANALYZE requires MySQL 8.0.18+ (server is {})",
                self.runtime.version
            )));
        }
        if analyze && self.read_only && !analyze_statement(sql).allowed_in_read_only() {
            return Err(MySqlError::message(
                "EXPLAIN ANALYZE executes the query and is blocked for write statements in Read-Only mode.",
            ));
        }

        let start = Instant::now();
        let explain_sql = if analyze {
            format!("EXPLAIN ANALYZE {sql}")
        } else {
            format!("EXPLAIN FORMAT=JSON {sql}")
        };
        let rows: Vec<String> = self
            .conn
            .query(explain_sql)
            .map_err(MySqlError::from_mysql)?;
        Ok(ExplainResult {
            is_analyze: analyze,
            raw_output: rows.join("\n"),
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }

    pub fn fetch_processlist(&mut self) -> Result<Vec<ProcessEntry>, MySqlError> {
        let rows: Vec<mysql::Row> = self
            .conn
            .query("SHOW FULL PROCESSLIST")
            .map_err(MySqlError::from_mysql)?;
        let mut entries = Vec::with_capacity(rows.len());
        for row in rows {
            let id: u64 = row.get(0).unwrap_or(0);
            let user: String = row.get(1).unwrap_or_default();
            let host: String = row.get(2).unwrap_or_default();
            let db: Option<String> = row.get(3).unwrap_or(None);
            let command: String = row.get(4).unwrap_or_default();
            let time_secs: i64 = row.get(5).unwrap_or(0);
            let state: Option<String> = row.get(6).unwrap_or(None);
            let info: Option<String> = row.get(7).unwrap_or(None);
            entries.push(ProcessEntry {
                id,
                user,
                host,
                db,
                command,
                time_secs,
                state,
                info,
            });
        }
        Ok(entries)
    }

    pub fn fetch_health_metrics(&mut self) -> Result<ServerHealthMetrics, MySqlError> {
        let rows: Vec<(String, String)> = self
            .conn
            .query(
                "SHOW GLOBAL STATUS WHERE Variable_name IN (\
                 'Uptime', 'Threads_connected', 'Threads_running', 'Questions', \
                 'Innodb_buffer_pool_read_requests', 'Innodb_buffer_pool_reads')",
            )
            .map_err(MySqlError::from_mysql)?;

        let mut metrics = ServerHealthMetrics::default();
        let mut bp_requests: u64 = 0;
        let mut bp_reads: u64 = 0;
        for (k, v) in rows {
            let num = v.parse::<u64>().unwrap_or(0);
            match k.as_str() {
                "Uptime" => metrics.uptime_secs = num,
                "Threads_connected" => metrics.threads_connected = num,
                "Threads_running" => metrics.threads_running = num,
                "Questions" => metrics.questions = num,
                "Innodb_buffer_pool_read_requests" => bp_requests = num,
                "Innodb_buffer_pool_reads" => bp_reads = num,
                _ => {}
            }
        }
        if bp_requests > 0 {
            let hit = 100.0 * (1.0 - (bp_reads as f64 / bp_requests as f64)).clamp(0.0, 1.0);
            metrics.innodb_hit_ratio_pct = Some(hit);
        }
        Ok(metrics)
    }

    pub fn kill_query_via_aux(
        profile: &ServerProfile,
        password: Option<&str>,
        target_conn_id: u32,
    ) -> Result<(), MySqlError> {
        let timeout = Duration::from_secs(profile.connect_timeout_secs.max(3) as u64);
        let (_tunnel, mut aux) = connect_with_tls_fallback(profile, password, timeout)?;
        aux.query_drop(format!("KILL QUERY {target_conn_id}"))
            .map_err(MySqlError::from_mysql)
    }

    fn sync_tx_state_after_stmt(&mut self, sql: &str) {
        let upper = sql.trim_start().to_ascii_uppercase();
        if upper.starts_with("START TRANSACTION") || upper.starts_with("BEGIN") {
            self.runtime.in_transaction = true;
        } else if upper.starts_with("COMMIT") || upper.starts_with("ROLLBACK") {
            self.runtime.in_transaction = false;
        } else if upper.starts_with("USE ") {
            if let Ok(Some(db)) = self.conn.query_first::<Option<String>, _>("SELECT DATABASE()") {
                self.runtime.current_schema = db;
            }
        }
    }
}

fn connect_with_tls_fallback(
    profile: &ServerProfile,
    password: Option<&str>,
    timeout: Duration,
) -> Result<(Option<SshTunnel>, Conn), MySqlError> {
    let (tunnel, opts) = build_opts(profile, password, timeout)?;
    match Conn::new(opts.clone()) {
        Ok(conn) => Ok((tunnel, conn)),
        Err(tls_err) if profile.tls == TlsMode::Preferred && is_tls_fallback_eligible(&tls_err) => {
            let plain_opts = opts.ssl_opts(None);
            match Conn::new(plain_opts) {
                Ok(conn) => Ok((tunnel, conn)),
                Err(mysql::Error::MySqlError(me)) if me.code == 3159 => {
                    Err(MySqlError::message(format!(
                        "Server requires TLS (ERROR 3159), and TLS handshake failed: {tls_err}"
                    )))
                }
                Err(plain_err) => Err(MySqlError::from_mysql(plain_err)),
            }
        }
        Err(err) => Err(MySqlError::from_mysql(err)),
    }
}

fn is_tls_fallback_eligible(err: &mysql::Error) -> bool {
    match err {
        mysql::Error::DriverError(mysql::DriverError::TlsNotSupported) => true,
        mysql::Error::TlsError(_) => true,
        mysql::Error::IoError(io_err) => {
            if matches!(
                io_err.kind(),
                std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::AddrNotAvailable
                    | std::io::ErrorKind::NotFound
            ) {
                return false;
            }
            let lower = io_err.to_string().to_ascii_lowercase();
            matches!(
                io_err.kind(),
                std::io::ErrorKind::InvalidData
                    | std::io::ErrorKind::UnexpectedEof
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
            ) || lower.contains("tls")
                || lower.contains("ssl")
                || lower.contains("handshake")
                || lower.contains("alert")
                || lower.contains("certificate")
                || lower.contains("cipher")
                || lower.contains("protocol")
        }
        _ => {
            let lower = err.to_string().to_ascii_lowercase();
            lower.contains("server does not have this capability")
                || lower.contains("tls")
                || lower.contains("ssl")
                || lower.contains("handshake")
        }
    }
}

fn build_opts(
    profile: &ServerProfile,
    password: Option<&str>,
    timeout: Duration,
) -> Result<(Option<SshTunnel>, OptsBuilder), MySqlError> {
    let mut builder = OptsBuilder::new()
        .user(Some(profile.username.clone()))
        .pass(password.map(ToOwned::to_owned))
        .db_name(profile.default_schema.clone().filter(|s| !s.trim().is_empty()))
        .tcp_connect_timeout(Some(timeout))
        .read_timeout(Some(Duration::from_secs(300)))
        .write_timeout(Some(Duration::from_secs(60)));

    let tunnel = match &profile.endpoint {
        ConnectionEndpoint::Tcp { host, port } => {
            if profile.ssh.enabled {
                let tun = SshTunnel::open(&profile.ssh, host, *port, timeout)
                    .map_err(MySqlError::message)?;
                builder = builder
                    .ip_or_hostname(Some("127.0.0.1"))
                    .tcp_port(tun.local_port());
                Some(tun)
            } else {
                builder = builder.ip_or_hostname(Some(host.clone())).tcp_port(*port);
                None
            }
        }
        ConnectionEndpoint::UnixSocket { path } => {
            builder = builder.socket(Some(path.clone()));
            None
        }
    };

    let ssl_opts = match profile.tls {
        TlsMode::Disabled => None,
        TlsMode::Preferred => Some(
            SslOpts::default()
                .with_danger_accept_invalid_certs(true)
                .with_danger_skip_domain_validation(true),
        ),
        TlsMode::Required => Some(
            SslOpts::default()
                .with_danger_accept_invalid_certs(true)
                .with_danger_skip_domain_validation(true),
        ),
        TlsMode::VerifyCa => {
            let mut s = SslOpts::default().with_danger_skip_domain_validation(true);
            if let Some(ca) = profile.ca_cert_path.as_deref().filter(|p| !p.trim().is_empty()) {
                s = s.with_root_cert_path(Some(PathBuf::from(ca)));
            }
            Some(s)
        }
        TlsMode::VerifyIdentity => {
            let mut s = SslOpts::default();
            if let Some(ca) = profile.ca_cert_path.as_deref().filter(|p| !p.trim().is_empty()) {
                s = s.with_root_cert_path(Some(PathBuf::from(ca)));
            }
            Some(s)
        }
    };

    builder = builder.ssl_opts(ssl_opts);
    Ok((tunnel, builder))
}

type RuntimeInfoRow = (String, u8, String, String, Option<String>, u32, u8);

fn load_runtime_info(conn: &mut Conn) -> Result<ServerRuntimeInfo, MySqlError> {
    let row: Option<RuntimeInfoRow> = conn
        .query_first(
            "SELECT VERSION(), @@lower_case_table_names, @@sql_mode, @@time_zone, \
             DATABASE(), CONNECTION_ID(), @@autocommit",
        )
        .map_err(MySqlError::from_mysql)?;

    let (version, casing, sql_mode, time_zone, current_schema, connection_id, autocommit) =
        row.unwrap_or_else(|| ("8.0.0".into(), 0, String::new(), "SYSTEM".into(), None, 0, 1));
    let (maj, min, pat) = ServerRuntimeInfo::parse_version_tuple(&version);

    Ok(ServerRuntimeInfo {
        version,
        version_major: maj,
        version_minor: min,
        version_patch: pat,
        lower_case_table_names: casing,
        sql_mode,
        time_zone,
        current_schema,
        connection_id,
        autocommit: autocommit != 0,
        in_transaction: false,
    })
}

fn format_mysql_value(val: Value, col_type: ColumnType, flags: ColumnFlags) -> CellValue {
    match val {
        Value::NULL => CellValue::Null,
        Value::Int(v) => CellValue::Text(v.to_string()),
        Value::UInt(v) => CellValue::Text(v.to_string()),
        Value::Float(v) => CellValue::Text(v.to_string()),
        Value::Double(v) => CellValue::Text(v.to_string()),
        Value::Date(y, m, d, h, min, s, us) => {
            if matches!(col_type, ColumnType::MYSQL_TYPE_DATE) {
                CellValue::Text(format!("{y:04}-{m:02}-{d:02}"))
            } else if us > 0 {
                CellValue::Text(format!("{y:04}-{m:02}-{d:02} {h:02}:{min:02}:{s:02}.{us:06}"))
            } else {
                CellValue::Text(format!("{y:04}-{m:02}-{d:02} {h:02}:{min:02}:{s:02}"))
            }
        }
        Value::Time(neg, days, h, m, s, us) => {
            let sign = if neg { "-" } else { "" };
            let total_h = days.saturating_mul(24).saturating_add(h as u32);
            if us > 0 {
                CellValue::Text(format!("{sign}{total_h:02}:{m:02}:{s:02}.{us:06}"))
            } else {
                CellValue::Text(format!("{sign}{total_h:02}:{m:02}:{s:02}"))
            }
        }
        Value::Bytes(bytes) => {
            if col_type == ColumnType::MYSQL_TYPE_BIT {
                let mut bits = String::with_capacity(bytes.len() * 8 + 3);
                bits.push_str("b'");
                for b in &bytes {
                    bits.push_str(&format!("{b:08b}"));
                }
                bits.push('\'');
                return CellValue::Text(bits);
            }
            if flags.contains(ColumnFlags::BINARY_FLAG)
                && matches!(
                    col_type,
                    ColumnType::MYSQL_TYPE_BLOB
                        | ColumnType::MYSQL_TYPE_TINY_BLOB
                        | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
                        | ColumnType::MYSQL_TYPE_LONG_BLOB
                        | ColumnType::MYSQL_TYPE_GEOMETRY
                )
            {
                let preview_len = bytes.len().min(64);
                let mut hex = String::with_capacity(2 + preview_len * 2 + 24);
                hex.push_str("0x");
                for b in &bytes[..preview_len] {
                    hex.push_str(&format!("{b:02X}"));
                }
                if bytes.len() > preview_len {
                    return CellValue::Truncated {
                        preview: hex,
                        total_bytes: bytes.len(),
                    };
                }
                return CellValue::Text(hex);
            }
            match String::from_utf8(bytes) {
                Ok(s) => CellValue::from_text(s),
                Err(e) => {
                    let raw = e.into_bytes();
                    let preview_len = raw.len().min(64);
                    let mut hex = String::from("0x");
                    for b in &raw[..preview_len] {
                        hex.push_str(&format!("{b:02X}"));
                    }
                    CellValue::Truncated {
                        preview: hex,
                        total_bytes: raw.len(),
                    }
                }
            }
        }
    }
}

fn is_numeric_mysql_type(t: ColumnType) -> bool {
    matches!(
        t,
        ColumnType::MYSQL_TYPE_TINY
            | ColumnType::MYSQL_TYPE_SHORT
            | ColumnType::MYSQL_TYPE_INT24
            | ColumnType::MYSQL_TYPE_LONG
            | ColumnType::MYSQL_TYPE_LONGLONG
            | ColumnType::MYSQL_TYPE_FLOAT
            | ColumnType::MYSQL_TYPE_DOUBLE
            | ColumnType::MYSQL_TYPE_DECIMAL
            | ColumnType::MYSQL_TYPE_NEWDECIMAL
            | ColumnType::MYSQL_TYPE_YEAR
    )
}

fn mysql_type_label(t: ColumnType, flags: ColumnFlags) -> String {
    let base = match t {
        ColumnType::MYSQL_TYPE_TINY => "tinyint",
        ColumnType::MYSQL_TYPE_SHORT => "smallint",
        ColumnType::MYSQL_TYPE_INT24 => "mediumint",
        ColumnType::MYSQL_TYPE_LONG => "int",
        ColumnType::MYSQL_TYPE_LONGLONG => "bigint",
        ColumnType::MYSQL_TYPE_FLOAT => "float",
        ColumnType::MYSQL_TYPE_DOUBLE => "double",
        ColumnType::MYSQL_TYPE_DECIMAL | ColumnType::MYSQL_TYPE_NEWDECIMAL => "decimal",
        ColumnType::MYSQL_TYPE_DATE => "date",
        ColumnType::MYSQL_TYPE_TIME | ColumnType::MYSQL_TYPE_TIME2 => "time",
        ColumnType::MYSQL_TYPE_DATETIME | ColumnType::MYSQL_TYPE_DATETIME2 => "datetime",
        ColumnType::MYSQL_TYPE_TIMESTAMP | ColumnType::MYSQL_TYPE_TIMESTAMP2 => "timestamp",
        ColumnType::MYSQL_TYPE_YEAR => "year",
        ColumnType::MYSQL_TYPE_VARCHAR | ColumnType::MYSQL_TYPE_VAR_STRING => "varchar",
        ColumnType::MYSQL_TYPE_STRING => "char",
        ColumnType::MYSQL_TYPE_JSON => "json",
        ColumnType::MYSQL_TYPE_BIT => "bit",
        ColumnType::MYSQL_TYPE_BLOB => "blob",
        ColumnType::MYSQL_TYPE_TINY_BLOB => "tinyblob",
        ColumnType::MYSQL_TYPE_MEDIUM_BLOB => "mediumblob",
        ColumnType::MYSQL_TYPE_LONG_BLOB => "longblob",
        ColumnType::MYSQL_TYPE_ENUM => "enum",
        ColumnType::MYSQL_TYPE_SET => "set",
        ColumnType::MYSQL_TYPE_GEOMETRY => "geometry",
        _ => "text",
    };
    if flags.contains(ColumnFlags::UNSIGNED_FLAG) && is_numeric_mysql_type(t) {
        format!("{base} unsigned")
    } else {
        base.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_tls_fallback_matches_handshake_failures_only() {
        let not_supported = mysql::Error::DriverError(mysql::DriverError::TlsNotSupported);
        assert!(is_tls_fallback_eligible(&not_supported));

        let alert = mysql::Error::IoError(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "received fatal alert: HandshakeFailure",
        ));
        assert!(is_tls_fallback_eligible(&alert));

        let refused = mysql::Error::IoError(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        ));
        assert!(!is_tls_fallback_eligible(&refused));
    }

    #[test]
    fn mysql_error_display_preserves_underlying_raw_detail() {
        let err = MySqlError::message("received fatal alert: ProtocolVersion (TLSv1.0)");
        let rendered = err.to_string();
        assert!(rendered.contains("ProtocolVersion"));
    }
}

use cottondata_core::{
    ConnectionEndpoint, DbDiagnostic, ServerProfile, ServerRuntimeInfo, SqlDialect, TlsMode, UNLIMITED_ROWS,
};
use cottondata_driver::{
    DatabaseSession, DbError, ExplainResult, ProcessEntry, QueryOutcome, SchemaObjects, SchemaSummary,
    ServerHealthMetrics, SshTunnel, TableDetail,
};
use cottondata_grid::{CellValue, ColumnMeta, ColumnarPage};
use cottondata_sql::{analyze_statement, has_explicit_row_limit, is_explainable_statement, is_pageable_read_query};
use mysql::consts::{ColumnFlags, ColumnType};
use mysql::prelude::Queryable;
use mysql::{Conn, OptsBuilder, SslOpts, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const CONNECTION_LOST_ERROR_CODES: [u16; 2] = [2006, 2013];
const CONNECTION_LOST_MESSAGES: [&str; 6] = [
    "server has gone away",
    "lost connection",
    "broken pipe",
    "connection reset",
    "unexpected eof",
    "closed connection",
];

pub(crate) fn mysql_error(err: mysql::Error) -> DbError {
    let code = match &err {
        mysql::Error::MySqlError(server_error) => Some(server_error.code),
        _ => None,
    };
    let diagnostic = DbDiagnostic::classify(err.to_string(), code.map(u32::from));
    let lower_message = diagnostic.raw_message.to_ascii_lowercase();
    let is_connection_lost = code.is_some_and(|value| CONNECTION_LOST_ERROR_CODES.contains(&value))
        || CONNECTION_LOST_MESSAGES.iter().any(|pattern| lower_message.contains(pattern));
    if is_connection_lost {
        DbError::connection_lost(diagnostic)
    } else {
        DbError::new(diagnostic)
    }
}

pub struct MySqlSession {
    conn: Conn,
    tunnel: Option<SshTunnel>,
    runtime: ServerRuntimeInfo,
    read_only: bool,
    active_select_limit: Option<u64>,
}

impl MySqlSession {
    pub fn connect(profile: &ServerProfile, password: Option<&str>) -> Result<Self, DbError> {
        let timeout = Duration::from_secs(profile.connect_timeout_secs.max(3) as u64);
        let (tunnel, mut conn) = connect_with_tls_fallback(profile, password, timeout)?;
        let runtime = load_runtime_info(&mut conn)?;
        Ok(Self {
            conn,
            tunnel,
            runtime,
            read_only: profile.read_only,
            active_select_limit: None,
        })
    }

    fn ensure_select_limit(&mut self, desired: Option<u64>) -> Result<(), DbError> {
        if self.active_select_limit == desired {
            return Ok(());
        }
        let stmt = match desired {
            Some(limit) => format!("SET SESSION sql_select_limit = {limit}"),
            None => "SET SESSION sql_select_limit = DEFAULT".to_string(),
        };
        self.conn.query_drop(stmt).map_err(mysql_error)?;
        self.active_select_limit = desired;
        Ok(())
    }

    pub fn conn_mut(&mut self) -> &mut Conn {
        &mut self.conn
    }

    pub fn kill_query_via_aux(
        profile: &ServerProfile,
        password: Option<&str>,
        target_conn_id: u32,
    ) -> Result<(), DbError> {
        let timeout = Duration::from_secs(profile.connect_timeout_secs.max(3) as u64);
        let (_tunnel, mut aux) = connect_with_tls_fallback(profile, password, timeout)?;
        aux.query_drop(format!("KILL QUERY {target_conn_id}"))
            .map_err(mysql_error)
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
        } else if upper.starts_with("SET ") {
            self.active_select_limit = None;
        }
    }

    fn ensure_writable(&self, sql: &str) -> Result<(), DbError> {
        let safety = analyze_statement(sql);
        if self.read_only && !safety.allowed_in_read_only() {
            return Err(DbError::message(
                "Blocked by server Read-Only protection. Disable Read-Only on this server slot to execute writes.",
            ));
        }
        Ok(())
    }
}

impl DatabaseSession for MySqlSession {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::MySql
    }

    fn runtime(&self) -> &ServerRuntimeInfo {
        &self.runtime
    }

    fn ping(&mut self) -> Result<(), DbError> {
        if let Some(tunnel) = &mut self.tunnel {
            if !tunnel.is_alive() {
                return Err(DbError::message("SSH tunnel process terminated"));
            }
        }
        self.conn.ping().map_err(mysql_error)
    }

    fn set_schema(&mut self, schema: &str) -> Result<(), DbError> {
        let escaped = schema.replace('`', "``");
        self.conn
            .query_drop(format!("USE `{escaped}`"))
            .map_err(mysql_error)?;
        self.runtime.current_schema = Some(schema.to_string());
        Ok(())
    }

    fn set_autocommit(&mut self, enabled: bool) -> Result<(), DbError> {
        let val = if enabled { 1 } else { 0 };
        self.conn
            .query_drop(format!("SET autocommit = {val}"))
            .map_err(mysql_error)?;
        self.runtime.autocommit = enabled;
        if enabled {
            self.runtime.in_transaction = false;
        }
        Ok(())
    }

    fn begin_transaction(&mut self) -> Result<(), DbError> {
        self.conn
            .query_drop("START TRANSACTION")
            .map_err(mysql_error)?;
        self.runtime.in_transaction = true;
        Ok(())
    }

    fn commit(&mut self) -> Result<(), DbError> {
        self.conn
            .query_drop("COMMIT")
            .map_err(mysql_error)?;
        self.runtime.in_transaction = false;
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), DbError> {
        self.conn
            .query_drop("ROLLBACK")
            .map_err(mysql_error)?;
        self.runtime.in_transaction = false;
        Ok(())
    }

    fn execute_sql(&mut self, sql: &str, max_rows: u32) -> Result<QueryOutcome, DbError> {
        self.ensure_writable(sql)?;

        let unlimited = max_rows == UNLIMITED_ROWS;
        let cap = if unlimited {
            usize::MAX
        } else {
            max_rows.max(1) as usize
        };
        let desired_server_limit = (!unlimited && is_pageable_read_query(sql) && !has_explicit_row_limit(sql))
            .then(|| u64::from(max_rows.max(1)).saturating_add(1));
        self.ensure_select_limit(desired_server_limit)?;

        let start = Instant::now();
        let mut result = self.conn.query_iter(sql).map_err(mysql_error)?;
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

        let (columns, col_types) = extract_mysql_columns(cols_slice);

        let mut page = ColumnarPage::new(columns, sql.to_string());
        let mut row_buf = Vec::with_capacity(col_types.len());

        for row_res in result.by_ref() {
            let row = row_res.map_err(mysql_error)?;
            if page.row_count >= cap {
                page.has_more_rows = true;
                break;
            }
            row_buf.clear();
            let mut values = row.unwrap().into_iter();
            for &(col_type, flags) in &col_types {
                let val = values.next().unwrap_or(Value::NULL);
                row_buf.push(format_mysql_value(val, col_type, flags));
            }
            page.push_row(&row_buf);
        }

        drop(result);
        page.shrink_to_fit();
        page.elapsed_ms = start.elapsed().as_millis() as u64;
        self.sync_tx_state_after_stmt(sql);
        Ok(QueryOutcome::Rows(page))
    }

    #[allow(clippy::type_complexity)]
    fn stream_query(
        &mut self,
        sql: &str,
        on_columns: &mut dyn FnMut(&[ColumnMeta]) -> Result<(), String>,
        on_row: &mut dyn FnMut(&[Option<String>]) -> Result<bool, String>,
    ) -> Result<u64, DbError> {
        self.ensure_writable(sql)?;
        self.ensure_select_limit(None)?;

        let mut result = self.conn.query_iter(sql).map_err(mysql_error)?;
        let (columns, col_types) = extract_mysql_columns(result.columns().as_ref());
        on_columns(&columns).map_err(DbError::message)?;

        let mut row_buf: Vec<Option<String>> = Vec::with_capacity(col_types.len());
        let mut rows_streamed: u64 = 0;

        for row_res in result.by_ref() {
            let row = row_res.map_err(mysql_error)?;
            row_buf.clear();
            let mut values = row.unwrap().into_iter();
            for &(col_type, flags) in &col_types {
                let val = values.next().unwrap_or(Value::NULL);
                row_buf.push(format_mysql_text(val, col_type, flags));
            }
            let keep_going = on_row(&row_buf).map_err(DbError::message)?;
            rows_streamed += 1;
            if !keep_going {
                break;
            }
        }

        drop(result);
        self.sync_tx_state_after_stmt(sql);
        Ok(rows_streamed)
    }

    fn execute_atomic_batch(&mut self, statements: &[String]) -> Result<u64, DbError> {
        if self.read_only {
            return Err(DbError::message("Cannot submit changes while Read-Only protection is enabled."));
        }
        if statements.is_empty() {
            return Ok(0);
        }
        self.ensure_select_limit(None)?;

        let start = Instant::now();
        let use_savepoint = self.runtime.in_transaction || !self.runtime.autocommit;
        if use_savepoint {
            self.conn
                .query_drop("SAVEPOINT cottondata_grid_batch")
                .map_err(mysql_error)?;
        } else {
            self.conn
                .query_drop("START TRANSACTION")
                .map_err(mysql_error)?;
        }

        for stmt in statements {
            let trimmed = stmt.trim_start();
            let upper_prefix = trimmed.get(..7).unwrap_or(trimmed).to_ascii_uppercase();
            let expects_row_match = upper_prefix.starts_with("DELETE ");
            let exec_res: Result<u64, DbError> = match self.conn.query_iter(stmt) {
                Ok(result) => Ok(result.affected_rows()),
                Err(err) => Err(mysql_error(err)),
            };
            match exec_res {
                Ok(0) if expects_row_match => {
                    if use_savepoint {
                        let _ = self.conn.query_drop("ROLLBACK TO SAVEPOINT cottondata_grid_batch");
                        let _ = self.conn.query_drop("RELEASE SAVEPOINT cottondata_grid_batch");
                    } else {
                        let _ = self.conn.query_drop("ROLLBACK");
                    }
                    return Err(DbError::message(
                        "Row was modified or deleted on the server before saving (0 rows matched). Reload and retry.",
                    ));
                }
                Ok(_) => {}
                Err(err) => {
                    if use_savepoint {
                        let _ = self.conn.query_drop("ROLLBACK TO SAVEPOINT cottondata_grid_batch");
                        let _ = self.conn.query_drop("RELEASE SAVEPOINT cottondata_grid_batch");
                    } else {
                        let _ = self.conn.query_drop("ROLLBACK");
                    }
                    return Err(err);
                }
            }
        }

        if use_savepoint {
            self.conn
                .query_drop("RELEASE SAVEPOINT cottondata_grid_batch")
                .map_err(mysql_error)?;
        } else {
            self.conn
                .query_drop("COMMIT")
                .map_err(mysql_error)?;
            self.runtime.in_transaction = false;
        }
        Ok(start.elapsed().as_millis() as u64)
    }

    fn explain(&mut self, sql: &str, analyze: bool) -> Result<ExplainResult, DbError> {
        if !is_explainable_statement(sql) {
            return Err(DbError::message(
                "EXPLAIN only supports SELECT, TABLE, INSERT, UPDATE, DELETE, REPLACE, or WITH statements. Use Run for SHOW / DESCRIBE / DDL statements.",
            ));
        }
        if analyze && !self.runtime.supports_explain_analyze() {
            return Err(DbError::message(format!(
                "EXPLAIN ANALYZE requires MySQL 8.0.18+ (server is {})",
                self.runtime.version
            )));
        }
        if analyze && self.read_only && !analyze_statement(sql).allowed_in_read_only() {
            return Err(DbError::message(
                "EXPLAIN ANALYZE executes the query and is blocked for write statements in Read-Only mode.",
            ));
        }
        self.ensure_select_limit(None)?;

        let start = Instant::now();
        let explain_sql = if analyze {
            format!("EXPLAIN ANALYZE {sql}")
        } else {
            format!("EXPLAIN FORMAT=JSON {sql}")
        };
        let rows: Vec<String> = self
            .conn
            .query(explain_sql)
            .map_err(mysql_error)?;
        Ok(ExplainResult {
            is_analyze: analyze,
            label: if analyze { "EXPLAIN ANALYZE" } else { "EXPLAIN FORMAT=JSON" }.into(),
            raw_output: rows.join("\n"),
            elapsed_ms: start.elapsed().as_millis() as u64,
        })
    }

    fn fetch_processlist(&mut self) -> Result<Vec<ProcessEntry>, DbError> {
        self.ensure_select_limit(None)?;
        let rows: Vec<mysql::Row> = self
            .conn
            .query("SHOW FULL PROCESSLIST")
            .map_err(mysql_error)?;
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

    fn fetch_health_metrics(&mut self) -> Result<ServerHealthMetrics, DbError> {
        self.ensure_select_limit(None)?;
        let rows: Vec<(String, String)> = self
            .conn
            .query(
                "SHOW GLOBAL STATUS WHERE Variable_name IN (\
                 'Uptime', 'Threads_connected', 'Threads_running', 'Questions', \
                 'Innodb_buffer_pool_read_requests', 'Innodb_buffer_pool_reads')",
            )
            .map_err(mysql_error)?;

        let mut status_values = std::collections::HashMap::new();
        for (name, value) in rows {
            status_values.insert(name, value.parse::<u64>().unwrap_or(0));
        }
        let value_of = |name: &str| status_values.get(name).copied().unwrap_or(0);
        let mut metrics = ServerHealthMetrics::default();
        metrics.push("Uptime", format!("{}s", value_of("Uptime")));
        metrics.push(
            "Threads",
            format!("{}/{} running", value_of("Threads_running"), value_of("Threads_connected")),
        );
        metrics.push("Questions", value_of("Questions").to_string());
        let buffer_pool_requests = value_of("Innodb_buffer_pool_read_requests");
        if buffer_pool_requests > 0 {
            let disk_reads = value_of("Innodb_buffer_pool_reads") as f64;
            let hit_ratio = 100.0 * (1.0 - disk_reads / buffer_pool_requests as f64).clamp(0.0, 1.0);
            metrics.push("InnoDB hit", format!("{hit_ratio:.2}%"));
        }
        Ok(metrics)
    }

    fn list_schemas(&mut self) -> Result<Vec<SchemaSummary>, DbError> {
        self.ensure_select_limit(None)?;
        crate::introspection::list_schemas(&mut self.conn)
    }

    fn load_schema_objects(&mut self, schema: &str) -> Result<SchemaObjects, DbError> {
        self.ensure_select_limit(None)?;
        crate::introspection::load_schema_objects(&mut self.conn, schema)
    }

    fn load_table_detail(&mut self, schema: &str, table: &str) -> Result<TableDetail, DbError> {
        self.ensure_select_limit(None)?;
        crate::introspection::load_table_detail(&mut self.conn, schema, table)
    }
}

fn connect_with_tls_fallback(
    profile: &ServerProfile,
    password: Option<&str>,
    timeout: Duration,
) -> Result<(Option<SshTunnel>, Conn), DbError> {
    let (tunnel, opts) = build_opts(profile, password, timeout)?;
    match Conn::new(opts.clone()) {
        Ok(conn) => Ok((tunnel, conn)),
        Err(tls_err) if profile.tls == TlsMode::Preferred && is_tls_fallback_eligible(&tls_err) => {
            let plain_opts = opts.ssl_opts(None);
            match Conn::new(plain_opts) {
                Ok(conn) => Ok((tunnel, conn)),
                Err(mysql::Error::MySqlError(me)) if me.code == 3159 => {
                    Err(DbError::message(format!(
                        "Server requires TLS (ERROR 3159), and TLS handshake failed: {tls_err}"
                    )))
                }
                Err(plain_err) => Err(mysql_error(plain_err)),
            }
        }
        Err(err) => Err(mysql_error(err)),
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
) -> Result<(Option<SshTunnel>, OptsBuilder), DbError> {
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
                    .map_err(DbError::message)?;
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
        ConnectionEndpoint::File { path } => {
            return Err(DbError::message(format!(
                "MySQL needs a host and port or a Unix socket, not the database file {path}"
            )));
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

fn load_runtime_info(conn: &mut Conn) -> Result<ServerRuntimeInfo, DbError> {
    let row: Option<RuntimeInfoRow> = conn
        .query_first(
            "SELECT VERSION(), @@lower_case_table_names, @@sql_mode, @@time_zone, \
             DATABASE(), CONNECTION_ID(), @@autocommit",
        )
        .map_err(mysql_error)?;

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

fn extract_mysql_columns(cols_slice: &[mysql::Column]) -> (Vec<ColumnMeta>, Vec<(ColumnType, ColumnFlags)>) {
    let columns = cols_slice
        .iter()
        .map(|c| {
            let flags = c.flags();
            let col_type = c.column_type();
            ColumnMeta {
                name: c.name_str().into_owned(),
                org_name: {
                    let o = c.org_name_str();
                    if o.is_empty() { None } else { Some(o.into_owned()) }
                },
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
                is_unique_key: flags.contains(ColumnFlags::UNIQUE_KEY_FLAG),
                is_nullable: !flags.contains(ColumnFlags::NOT_NULL_FLAG),
                is_auto_increment: flags.contains(ColumnFlags::AUTO_INCREMENT_FLAG),
                is_numeric: is_numeric_mysql_type(col_type),
                is_binary: flags.contains(ColumnFlags::BINARY_FLAG),
            }
        })
        .collect();

    let col_types = cols_slice
        .iter()
        .map(|c| (c.column_type(), c.flags()))
        .collect();

    (columns, col_types)
}

fn format_mysql_bit(bytes: &[u8]) -> String {
    let mut bits = String::with_capacity(bytes.len() * 8 + 3);
    bits.push_str("b'");
    for b in bytes {
        bits.push_str(&format!("{b:08b}"));
    }
    bits.push('\'');
    bits
}

fn format_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(2 + bytes.len() * 2);
    hex.push_str("0x");
    for b in bytes {
        hex.push_str(&format!("{b:02X}"));
    }
    hex
}

fn is_binary_blob(col_type: ColumnType, flags: ColumnFlags) -> bool {
    flags.contains(ColumnFlags::BINARY_FLAG)
        && matches!(
            col_type,
            ColumnType::MYSQL_TYPE_BLOB
                | ColumnType::MYSQL_TYPE_TINY_BLOB
                | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
                | ColumnType::MYSQL_TYPE_LONG_BLOB
                | ColumnType::MYSQL_TYPE_GEOMETRY
        )
}

fn format_mysql_text(val: Value, col_type: ColumnType, flags: ColumnFlags) -> Option<String> {
    match val {
        Value::NULL => None,
        Value::Int(v) => Some(v.to_string()),
        Value::UInt(v) => Some(v.to_string()),
        Value::Float(v) => Some(v.to_string()),
        Value::Double(v) => Some(v.to_string()),
        Value::Date(y, m, d, h, min, s, us) => {
            if matches!(col_type, ColumnType::MYSQL_TYPE_DATE) {
                Some(format!("{y:04}-{m:02}-{d:02}"))
            } else if us > 0 {
                Some(format!("{y:04}-{m:02}-{d:02} {h:02}:{min:02}:{s:02}.{us:06}"))
            } else {
                Some(format!("{y:04}-{m:02}-{d:02} {h:02}:{min:02}:{s:02}"))
            }
        }
        Value::Time(neg, days, h, m, s, us) => {
            let sign = if neg { "-" } else { "" };
            let total_h = days.saturating_mul(24).saturating_add(h as u32);
            if us > 0 {
                Some(format!("{sign}{total_h:02}:{m:02}:{s:02}.{us:06}"))
            } else {
                Some(format!("{sign}{total_h:02}:{m:02}:{s:02}"))
            }
        }
        Value::Bytes(bytes) => {
            if col_type == ColumnType::MYSQL_TYPE_BIT {
                return Some(format_mysql_bit(&bytes));
            }
            if is_binary_blob(col_type, flags) {
                return Some(format_hex(&bytes));
            }
            match String::from_utf8(bytes) {
                Ok(s) => Some(s),
                Err(e) => Some(format_hex(&e.into_bytes())),
            }
        }
    }
}

fn format_mysql_value(val: Value, col_type: ColumnType, flags: ColumnFlags) -> CellValue {
    match val {
        Value::NULL => CellValue::Null,
        Value::Bytes(bytes) => {
            if col_type == ColumnType::MYSQL_TYPE_BIT {
                return CellValue::Text(format_mysql_bit(&bytes));
            }
            if is_binary_blob(col_type, flags) {
                let preview_len = bytes.len().min(64);
                let hex = format_hex(&bytes[..preview_len]);
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
                    CellValue::Truncated {
                        preview: format_hex(&raw[..preview_len]),
                        total_bytes: raw.len(),
                    }
                }
            }
        }
        other => format_mysql_text(other, col_type, flags)
            .map(CellValue::Text)
            .unwrap_or(CellValue::Null),
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
    fn classifies_lost_connection_codes_and_messages() {
        let gone_away = mysql::Error::IoError(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "MySQL server has gone away",
        ));
        assert!(mysql_error(gone_away).is_connection_lost);

        let refused = mysql::Error::IoError(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        ));
        assert!(!mysql_error(refused).is_connection_lost);
    }

    #[test]
    fn format_mysql_text_preserves_full_values_without_preview_truncation() {
        assert_eq!(
            format_mysql_text(Value::NULL, ColumnType::MYSQL_TYPE_LONG, ColumnFlags::empty()),
            None
        );
        assert_eq!(
            format_mysql_text(Value::Int(-7), ColumnType::MYSQL_TYPE_LONG, ColumnFlags::empty()),
            Some("-7".into())
        );
        assert_eq!(
            format_mysql_text(
                Value::Date(2026, 11, 26, 0, 0, 0, 0),
                ColumnType::MYSQL_TYPE_DATE,
                ColumnFlags::empty(),
            ),
            Some("2026-11-26".into())
        );
        assert_eq!(
            format_mysql_text(
                Value::Bytes(vec![0b0000_0101]),
                ColumnType::MYSQL_TYPE_BIT,
                ColumnFlags::empty(),
            ),
            Some("b'00000101'".into())
        );
        let long_text = "x".repeat(8192);
        assert_eq!(
            format_mysql_text(
                Value::Bytes(long_text.as_bytes().to_vec()),
                ColumnType::MYSQL_TYPE_VARCHAR,
                ColumnFlags::empty(),
            ),
            Some(long_text)
        );
        let blob = vec![0xAB; 100];
        let hex = format_mysql_text(
            Value::Bytes(blob),
            ColumnType::MYSQL_TYPE_BLOB,
            ColumnFlags::BINARY_FLAG,
        )
        .expect("hex output");
        assert_eq!(hex.len(), 2 + 200);
        assert!(hex.starts_with("0xABAB"));
    }
}

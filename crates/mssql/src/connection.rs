use crate::config::build_config;
use crate::errors::mssql_error;
use crate::rows::{int, optional_text, text, MssqlClient};
use cottondata_core::{ServerProfile, ServerRuntimeInfo};
use cottondata_driver::DbError;
use std::time::Duration;
use tiberius::Client;
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;

pub(crate) async fn open_client(
    profile: &ServerProfile,
    password: Option<&str>,
    host: &str,
    port: u16,
    timeout: Duration,
) -> Result<MssqlClient, DbError> {
    let connect = async {
        match connect_once(profile, password, host, port).await {
            Err(tiberius::error::Error::Routing {
                host: routed_host,
                port: routed_port,
            }) => connect_once(profile, password, &routed_host, routed_port)
                .await
                .map_err(mssql_error),
            other => other.map_err(mssql_error),
        }
    };
    tokio::time::timeout(timeout, connect)
        .await
        .map_err(|_| DbError::message(format!("Timed out connecting to SQL Server at {host}:{port}")))?
}

async fn connect_once(
    profile: &ServerProfile,
    password: Option<&str>,
    host: &str,
    port: u16,
) -> Result<MssqlClient, tiberius::error::Error> {
    let config = build_config(profile, password, host, port);
    let tcp = TcpStream::connect(config.get_addr()).await?;
    tcp.set_nodelay(true)?;
    Client::connect(config, tcp.compat_write()).await
}

pub(crate) async fn load_runtime_info(client: &mut MssqlClient) -> Result<ServerRuntimeInfo, DbError> {
    let row = client
        .simple_query(
            "SELECT CAST(SERVERPROPERTY('ProductVersion') AS nvarchar(128)), @@SPID, DB_NAME(), \
                    CAST(SERVERPROPERTY('Edition') AS nvarchar(128)), CAST(@@OPTIONS & 2 AS int)",
        )
        .await
        .map_err(mssql_error)?
        .into_row()
        .await
        .map_err(mssql_error)?
        .ok_or_else(|| DbError::message("SQL Server did not return server information"))?;
    let version = text(&row, 0);
    let (version_major, version_minor, version_patch) = ServerRuntimeInfo::parse_version_tuple(&version);
    Ok(ServerRuntimeInfo {
        version,
        version_major,
        version_minor,
        version_patch,
        lower_case_table_names: 0,
        sql_mode: text(&row, 3),
        time_zone: String::new(),
        current_schema: optional_text(&row, 2),
        connection_id: u32::try_from(int(&row, 1)).unwrap_or(0),
        autocommit: int(&row, 4) == 0,
        in_transaction: false,
    })
}

use cottondata_core::{ConnectionEndpoint, DatabaseEngine, ServerProfile, TlsMode};
use cottondata_driver::{DatabaseSession, QueryOutcome};
use cottondata_mssql::MssqlSession;

struct LiveTarget {
    profile: ServerProfile,
    password: String,
}

fn live_target() -> Option<LiveTarget> {
    let host = std::env::var("COTTONDATA_MSSQL_HOST").ok()?;
    let port = std::env::var("COTTONDATA_MSSQL_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1433);
    let user = std::env::var("COTTONDATA_MSSQL_USER").ok()?;
    let password = std::env::var("COTTONDATA_MSSQL_PASSWORD").ok()?;
    let mut profile = ServerProfile::new_local("live-mssql", host.clone(), port, user);
    profile.engine = DatabaseEngine::SqlServer;
    profile.endpoint = ConnectionEndpoint::Tcp { host, port };
    profile.ssh.enabled = false;
    profile.tls = TlsMode::Preferred;
    profile.read_only = true;
    profile.default_schema = std::env::var("COTTONDATA_MSSQL_DATABASE").ok();
    Some(LiveTarget { profile, password })
}

#[test]
fn read_only_round_trip_against_live_sql_server() {
    let Some(target) = live_target() else {
        eprintln!("skipped: set COTTONDATA_MSSQL_HOST, _USER and _PASSWORD to run against a live server");
        return;
    };
    let mut session = MssqlSession::connect(&target.profile, Some(&target.password)).expect("connect");
    assert!(!session.runtime().version.is_empty());

    let outcome = session
        .execute_sql(
            "SELECT 1 AS one, N'héllo' AS greeting, CAST(12.50 AS decimal(5,2)) AS amount",
            10,
        )
        .expect("select");
    let QueryOutcome::Rows(page) = outcome else {
        panic!("expected rows");
    };
    assert_eq!(page.row_count, 1);
    assert_eq!(page.cell_str(0, 0), Some("1"));
    assert_eq!(page.cell_str(0, 1), Some("héllo"));
    assert_eq!(page.cell_str(0, 2), Some("12.50"));

    let schemas = session.list_schemas().expect("list databases");
    assert!(schemas.iter().any(|schema| schema.name.eq_ignore_ascii_case("master")));

    if let Some(database) = session.runtime().current_schema.clone() {
        session.load_schema_objects(&database).expect("load objects");
    }
    let blocked = session.execute_sql("DELETE FROM missing_table", 1);
    assert!(blocked.is_err(), "read-only profile must block writes");
}

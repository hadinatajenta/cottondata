use cottondata_core::{ConnectionEndpoint, DatabaseEngine, ServerProfile};
use cottondata_driver::{DatabaseSession, QueryOutcome};
use cottondata_grid::ColumnarPage;
use cottondata_sqlite::SqliteSession;
use std::time::{Duration, Instant};

fn temp_profile(read_only: bool) -> (ServerProfile, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "cottondata-sqlite-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default()
    ));
    let mut profile = ServerProfile::new_local("sqlite-test", "127.0.0.1", 0, "");
    profile.engine = DatabaseEngine::Sqlite;
    profile.endpoint = ConnectionEndpoint::File {
        path: path.to_string_lossy().into_owned(),
    };
    profile.read_only = read_only;
    (profile, path)
}

fn rows(session: &mut SqliteSession, sql: &str, max_rows: u32) -> ColumnarPage {
    match session.execute_sql(sql, max_rows).expect(sql) {
        QueryOutcome::Rows(page) => page,
        QueryOutcome::Affected { .. } => panic!("expected rows from {sql}"),
    }
}

fn count_items(session: &mut SqliteSession) -> String {
    rows(session, "SELECT count(*) FROM items", 1)
        .cell_str(0, 0)
        .unwrap_or_default()
        .to_string()
}

#[test]
fn executes_reads_writes_limits_and_editable_metadata() {
    let (profile, path) = temp_profile(false);
    let mut session = SqliteSession::connect(&profile, None).expect("open");
    session
        .execute_sql(
            "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL, price REAL, data BLOB)",
            1,
        )
        .expect("create");
    let QueryOutcome::Affected { affected_rows, .. } = session
        .execute_sql(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 30) \
             INSERT INTO items (name, price, data) SELECT 'item ' || i, i * 1.5, x'0aff' FROM n",
            1,
        )
        .expect("insert")
    else {
        panic!("expected affected");
    };
    assert_eq!(affected_rows, 30);

    let page = rows(&mut session, "SELECT id, name, price, data, upper(name) AS shout FROM items", 10);
    assert_eq!(page.row_count, 10);
    assert!(page.has_more_rows);
    assert_eq!(page.cell_str(0, 2), Some("1.5"));
    assert_eq!(page.cell_str(0, 3), Some("0x0AFF"));
    let id = &page.columns[0];
    assert!(id.is_primary_key && id.is_auto_increment && id.is_numeric);
    assert_eq!(id.table.as_deref(), Some("items"));
    assert_eq!(page.columns[1].org_name.as_deref(), Some("name"));
    assert!(!page.columns[1].is_nullable);
    assert!(page.columns[3].is_binary);
    assert!(page.columns[4].table.is_none());

    let returned = rows(&mut session, "INSERT INTO items (name) VALUES ('a'), ('b') RETURNING id", 1);
    assert_eq!(returned.row_count, 1);
    assert_eq!(count_items(&mut session), "32", "RETURNING inserts must not be cut at the page limit");

    let QueryOutcome::Affected { affected_rows, .. } = session
        .execute_sql("UPDATE items SET price = 0 WHERE id <= 2; DELETE FROM items WHERE id = 3", 1)
        .expect("script")
    else {
        panic!("expected affected");
    };
    assert_eq!(affected_rows, 3);

    session.set_autocommit(false).expect("manual");
    session.execute_sql("DELETE FROM items", 1).expect("delete");
    assert!(session.runtime().in_transaction);
    session.rollback().expect("rollback");
    session.set_autocommit(true).expect("auto");
    assert_eq!(count_items(&mut session), "31");

    assert!(session
        .execute_atomic_batch(&["DELETE FROM items".into(), "INSERT INTO nope VALUES (1)".into()])
        .is_err());
    assert_eq!(count_items(&mut session), "31");

    let analyzed = session.explain("DELETE FROM items", true).expect("explain");
    assert!(analyzed.raw_output.contains("rolled back"));
    assert_eq!(count_items(&mut session), "31");
    let plan = session.explain("SELECT * FROM items WHERE id = 1", false).expect("plan");
    assert!(plan.raw_output.contains("SEARCH"), "{}", plan.raw_output);

    let mut streamed = 0;
    session
        .stream_query("SELECT * FROM items", &mut |_| Ok(()), &mut |_| {
            streamed += 1;
            Ok(true)
        })
        .expect("stream");
    assert_eq!(streamed, 31);
    assert!(!session.fetch_health_metrics().expect("health").metrics.is_empty());
    assert!(session.kill_process(1).is_err());
    let syntax = session.execute_sql("SELEC 1", 1).expect_err("syntax error");
    assert!(syntax.to_string().contains("syntax"));
    drop(session);

    let (mut read_only_profile, _) = temp_profile(true);
    read_only_profile.endpoint = profile.endpoint.clone();
    let mut read_only = SqliteSession::connect(&read_only_profile, None).expect("read only");
    assert!(read_only.execute_sql("DELETE FROM items", 1).is_err());
    assert!(read_only.execute_sql("SELECT * FROM items", 1).is_ok());
    drop(read_only);
    let _ = std::fs::remove_file(path);
}

#[test]
fn interrupt_handle_cancels_a_long_running_query() {
    let (profile, path) = temp_profile(false);
    let mut session = SqliteSession::connect(&profile, None).expect("open");
    let connection_id = session.runtime().connection_id;
    let cancel_profile = profile.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        SqliteSession::interrupt_running_query(&cancel_profile, connection_id)
    });
    let started = Instant::now();
    let outcome = session.execute_sql(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n) SELECT count(*) FROM n",
        1,
    );
    canceller.join().expect("thread").expect("interrupt");
    let error = outcome.expect_err("interrupted");
    assert_eq!(error.diagnostic.summary, "Query cancelled");
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(session);
    let _ = std::fs::remove_file(path);
}

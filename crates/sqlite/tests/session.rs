use cottondata_core::{ConnectionEndpoint, DatabaseEngine, ServerProfile, SqlDialect, UNLIMITED_ROWS};
use cottondata_driver::{DatabaseSession, QueryOutcome};
use cottondata_grid::{format_keyset_literal, ColumnarPage, KeysetSeek, PageFilterSort, SortDirection};
use cottondata_sql::{build_paginated_query, build_sorted_query};
use cottondata_sqlite::SqliteSession;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static TEMP_DB_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_profile(read_only: bool) -> (ServerProfile, std::path::PathBuf) {
    let seq = TEMP_DB_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "cottondata-sqlite-{}-{}-{seq}.db",
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

#[test]
fn supports_auto_limit_append_keyset_seek_and_unlimited_mode() {
    let (profile, path) = temp_profile(false);
    let mut session = SqliteSession::connect(&profile, None).expect("open");
    session
        .execute_sql("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL)", 1)
        .expect("create");
    session
        .execute_sql(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 30) \
             INSERT INTO users (name) SELECT 'user_' || i FROM n",
            1,
        )
        .expect("insert");

    let base_sql = "SELECT * FROM users ORDER BY id ASC";
    let mut console_page = rows(&mut session, base_sql, 10);
    assert_eq!(console_page.row_count, 10);
    assert!(console_page.has_more_rows);

    while console_page.has_more_rows {
        let next_sql = build_paginated_query(SqlDialect::Sqlite, base_sql, 11, console_page.row_count as u64)
            .expect("continuation sql");
        let next_chunk = rows(&mut session, &next_sql, 10);
        console_page.append_page(&next_chunk).expect("append");
    }
    assert_eq!(console_page.row_count, 30);
    assert!(!console_page.has_more_rows);
    assert_eq!(console_page.cell_str(0, 0), Some("1"));
    assert_eq!(console_page.cell_str(29, 0), Some("30"));

    let filter = PageFilterSort {
        where_clause: String::new(),
        order_by: None,
        page_index: 0,
        page_size: 10,
    };
    let first_table_sql = filter.build_select_sql_with_default_order(SqlDialect::Sqlite, "main", "users", Some("id"));
    let mut keyset_page = rows(&mut session, &first_table_sql, 10);
    assert_eq!(keyset_page.row_count, 10);
    assert!(keyset_page.has_more_rows);

    while keyset_page.has_more_rows {
        let last_id = keyset_page.cell_str(keyset_page.row_count - 1, 0).expect("last id");
        let literal = format_keyset_literal(SqlDialect::Sqlite, &keyset_page.columns[0], last_id).expect("literal");
        let seek = KeysetSeek {
            column: "id".into(),
            direction: SortDirection::Asc,
            last_value_sql: literal,
        };
        let next_sql = filter.build_keyset_select_sql(SqlDialect::Sqlite, "main", "users", &seek, 11);
        let next_chunk = rows(&mut session, &next_sql, 10);
        keyset_page.append_page(&next_chunk).expect("append keyset");
    }
    assert_eq!(keyset_page.row_count, 30);
    assert!(!keyset_page.has_more_rows);
    for idx in 0..30 {
        assert_eq!(keyset_page.cell_str(idx, 0), Some((idx + 1).to_string().as_str()));
    }

    let unlimited = rows(&mut session, "SELECT * FROM users", UNLIMITED_ROWS);
    assert_eq!(unlimited.row_count, 30);
    assert!(!unlimited.has_more_rows);

    drop(session);
    let _ = std::fs::remove_file(path);
}

#[test]
fn server_side_sorted_continuation_and_in_memory_column_sort() {
    let (profile, path) = temp_profile(false);
    let mut session = SqliteSession::connect(&profile, None).expect("open");
    session
        .execute_sql(
            "CREATE TABLE scores (id INTEGER PRIMARY KEY, label TEXT, score REAL)",
            1,
        )
        .expect("create");
    session
        .execute_sql(
            "INSERT INTO scores (id, label, score) VALUES \
             (1, 'charlie', 10.5), (2, 'Alpha', 2.0), (3, 'beta', NULL), (4, 'delta', -3.5), \
             (5, 'alpha', 100.0), (6, 'Echo', 25.0), (7, 'foxtrot', 7.25), (8, 'Golf', NULL), \
             (9, 'hotel', 0.0), (10, 'India', 50.0), (11, 'juliet', 15.0), (12, 'Kilo', 3.0)",
            1,
        )
        .expect("insert");

    let base_sql = "SELECT id, label, score FROM scores ORDER BY id ASC;";
    let desc_first_sql =
        build_sorted_query(SqlDialect::Sqlite, base_sql, "score", true, 6, 0).expect("desc sql");
    let mut desc_page = rows(&mut session, &desc_first_sql, 5);
    assert_eq!(desc_page.row_count, 5);
    assert!(desc_page.has_more_rows);

    while desc_page.has_more_rows {
        let next_sql = build_sorted_query(
            SqlDialect::Sqlite,
            base_sql,
            "score",
            true,
            6,
            desc_page.row_count as u64,
        )
        .expect("desc continuation");
        let chunk = rows(&mut session, &next_sql, 5);
        desc_page.append_page(&chunk).expect("append desc");
    }
    assert_eq!(desc_page.row_count, 12);
    assert!(!desc_page.has_more_rows);
    let desc_scores: Vec<Option<&str>> = (0..desc_page.row_count)
        .map(|r| desc_page.cell_str(r, 2))
        .collect();
    assert_eq!(
        desc_scores,
        vec![
            Some("100.0"),
            Some("50.0"),
            Some("25.0"),
            Some("15.0"),
            Some("10.5"),
            Some("7.25"),
            Some("3.0"),
            Some("2.0"),
            Some("0.0"),
            Some("-3.5"),
            None,
            None,
        ]
    );

    let cte_sql = "WITH filtered AS (SELECT id, label, score FROM scores WHERE score IS NOT NULL) SELECT * FROM filtered";
    let asc_cte_sql =
        build_sorted_query(SqlDialect::Sqlite, cte_sql, "score", false, 5, 0).expect("asc cte sql");
    let mut asc_page = rows(&mut session, &asc_cte_sql, 4);
    assert_eq!(asc_page.row_count, 4);
    assert!(asc_page.has_more_rows);
    while asc_page.has_more_rows {
        let next_sql = build_sorted_query(
            SqlDialect::Sqlite,
            cte_sql,
            "score",
            false,
            5,
            asc_page.row_count as u64,
        )
        .expect("asc continuation");
        let chunk = rows(&mut session, &next_sql, 4);
        asc_page.append_page(&chunk).expect("append asc");
    }
    assert_eq!(asc_page.row_count, 10);
    assert_eq!(asc_page.cell_str(0, 2), Some("-3.5"));
    assert_eq!(asc_page.cell_str(9, 2), Some("100.0"));

    let mut full_page = rows(&mut session, "SELECT id, label, score FROM scores ORDER BY id ASC", UNLIMITED_ROWS);
    let label_asc_idx = full_page.sorted_row_indices(1, SortDirection::Asc);
    assert_eq!(full_page.cell_str(label_asc_idx[0], 1), Some("Alpha"));
    assert_eq!(full_page.cell_str(label_asc_idx[1], 1), Some("alpha"));
    assert_eq!(full_page.cell_str(label_asc_idx[2], 1), Some("beta"));

    full_page.sort_by_column(2, SortDirection::Asc);
    assert_eq!(full_page.cell_str(0, 2), Some("-3.5"));
    assert_eq!(full_page.cell_str(9, 2), Some("100.0"));
    assert!(full_page.is_null(10, 2));
    assert!(full_page.is_null(11, 2));

    full_page.sort_by_column(2, SortDirection::Desc);
    assert_eq!(full_page.cell_str(0, 2), Some("100.0"));
    assert_eq!(full_page.cell_str(9, 2), Some("-3.5"));
    assert!(full_page.is_null(10, 2));
    assert!(full_page.is_null(11, 2));

    drop(session);
    let _ = std::fs::remove_file(path);
}

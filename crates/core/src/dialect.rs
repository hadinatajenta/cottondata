use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum DatabaseEngine {
    #[default]
    MySql,
    SqlServer,
    PostgreSql,
    CockroachDb,
    Sqlite,
}

impl DatabaseEngine {
    pub const ALL: [DatabaseEngine; 5] = [
        Self::MySql,
        Self::PostgreSql,
        Self::SqlServer,
        Self::CockroachDb,
        Self::Sqlite,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::MySql => "MySQL",
            Self::SqlServer => "SQL Server",
            Self::PostgreSql => "PostgreSQL",
            Self::CockroachDb => "CockroachDB",
            Self::Sqlite => "SQLite",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Self::MySql => 3306,
            Self::SqlServer => 1433,
            Self::PostgreSql => 5432,
            Self::CockroachDb => 26257,
            Self::Sqlite => 0,
        }
    }

    pub fn default_username(self) -> &'static str {
        match self {
            Self::MySql | Self::CockroachDb => "root",
            Self::SqlServer => "sa",
            Self::PostgreSql => "postgres",
            Self::Sqlite => "",
        }
    }

    pub fn dialect(self) -> SqlDialect {
        match self {
            Self::MySql => SqlDialect::MySql,
            Self::SqlServer => SqlDialect::SqlServer,
            Self::PostgreSql => SqlDialect::PostgreSql,
            Self::CockroachDb => SqlDialect::CockroachDb,
            Self::Sqlite => SqlDialect::Sqlite,
        }
    }

    pub fn supports_unix_socket(self) -> bool {
        matches!(self, Self::MySql | Self::PostgreSql)
    }

    pub fn is_file_based(self) -> bool {
        matches!(self, Self::Sqlite)
    }

    pub fn requires_credentials(self) -> bool {
        !self.is_file_based()
    }

    pub fn database_label(self) -> &'static str {
        match self {
            Self::MySql => "Default Schema",
            Self::SqlServer | Self::PostgreSql | Self::CockroachDb => "Database",
            Self::Sqlite => "Database File",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SqlDialect {
    #[default]
    MySql,
    SqlServer,
    PostgreSql,
    CockroachDb,
    Sqlite,
}

impl SqlDialect {
    pub fn is_postgres_family(self) -> bool {
        matches!(self, Self::PostgreSql | Self::CockroachDb)
    }

    pub fn quote_ident(self, ident: &str) -> String {
        match self {
            Self::MySql => format!("`{}`", ident.replace('`', "``")),
            Self::SqlServer => format!("[{}]", ident.replace(']', "]]")),
            Self::PostgreSql | Self::CockroachDb | Self::Sqlite => format!("\"{}\"", ident.replace('"', "\"\"")),
        }
    }

    pub fn qualified_table(self, schema: &str, table: &str) -> String {
        match self {
            Self::SqlServer => {
                let (owner, object) = table.split_once('.').unwrap_or(("dbo", table));
                format!(
                    "{}.{}.{}",
                    self.quote_ident(schema),
                    self.quote_ident(owner),
                    self.quote_ident(object)
                )
            }
            Self::MySql | Self::PostgreSql | Self::CockroachDb | Self::Sqlite if schema.is_empty() => {
                self.quote_ident(table)
            }
            Self::MySql | Self::PostgreSql | Self::CockroachDb | Self::Sqlite => {
                format!("{}.{}", self.quote_ident(schema), self.quote_ident(table))
            }
        }
    }

    pub fn string_literal(self, value: &str) -> String {
        match self {
            Self::MySql => format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''")),
            Self::SqlServer => format!("N'{}'", value.replace('\'', "''")),
            Self::PostgreSql | Self::CockroachDb | Self::Sqlite => format!("'{}'", value.replace('\'', "''")),
        }
    }

    pub fn binary_literal(self, hex_digits: &str) -> String {
        match self {
            Self::MySql | Self::SqlServer => format!("0x{hex_digits}"),
            Self::PostgreSql | Self::CockroachDb => format!("'\\x{hex_digits}'::bytea"),
            Self::Sqlite => format!("X'{hex_digits}'"),
        }
    }

    pub fn boolean_literal(self, value: bool) -> &'static str {
        match (self, value) {
            (Self::SqlServer | Self::Sqlite, true) => "1",
            (Self::SqlServer | Self::Sqlite, false) => "0",
            (Self::MySql | Self::PostgreSql | Self::CockroachDb, true) => "TRUE",
            (Self::MySql | Self::PostgreSql | Self::CockroachDb, false) => "FALSE",
        }
    }

    pub fn delete_single_row(self, table_ref: &str, predicate: &str) -> String {
        match self {
            Self::MySql | Self::CockroachDb => format!("DELETE FROM {table_ref} WHERE {predicate} LIMIT 1"),
            Self::SqlServer => format!("DELETE TOP (1) FROM {table_ref} WHERE {predicate}"),
            Self::PostgreSql | Self::Sqlite => format!("DELETE FROM {table_ref} WHERE {predicate}"),
        }
    }

    pub fn update_single_row(self, table_ref: &str, assignments: &str, predicate: &str) -> String {
        match self {
            Self::MySql | Self::CockroachDb => {
                format!("UPDATE {table_ref} SET {assignments} WHERE {predicate} LIMIT 1")
            }
            Self::SqlServer => format!("UPDATE TOP (1) {table_ref} SET {assignments} WHERE {predicate}"),
            Self::PostgreSql | Self::Sqlite => format!("UPDATE {table_ref} SET {assignments} WHERE {predicate}"),
        }
    }

    pub fn insert_default_values(self, table_ref: &str) -> String {
        match self {
            Self::MySql => format!("INSERT INTO {table_ref} () VALUES ()"),
            Self::SqlServer | Self::PostgreSql | Self::CockroachDb | Self::Sqlite => {
                format!("INSERT INTO {table_ref} DEFAULT VALUES")
            }
        }
    }

    pub fn paged_select(
        self,
        table_ref: &str,
        where_clause: Option<&str>,
        order_by: Option<&str>,
        limit: u32,
        offset: u64,
    ) -> String {
        let where_sql = where_clause
            .map(|clause| format!(" WHERE {clause}"))
            .unwrap_or_default();
        match self {
            Self::SqlServer => {
                let order_sql = order_by.unwrap_or("(SELECT NULL)");
                format!(
                    "SELECT * FROM {table_ref}{where_sql} ORDER BY {order_sql} OFFSET {offset} ROWS FETCH NEXT {limit} ROWS ONLY"
                )
            }
            Self::MySql | Self::PostgreSql | Self::CockroachDb | Self::Sqlite => {
                let order_sql = order_by.map(|order| format!(" ORDER BY {order}")).unwrap_or_default();
                format!("SELECT * FROM {table_ref}{where_sql}{order_sql} LIMIT {limit} OFFSET {offset}")
            }
        }
    }

    pub fn select_single_value(self, column_ref: &str, table_ref: &str, predicate: &str) -> String {
        match self {
            Self::SqlServer => format!("SELECT TOP (1) {column_ref} FROM {table_ref} WHERE {predicate}"),
            Self::MySql | Self::PostgreSql | Self::CockroachDb | Self::Sqlite => {
                format!("SELECT {column_ref} FROM {table_ref} WHERE {predicate} LIMIT 1")
            }
        }
    }

    pub fn kill_process(self, process_id: u64) -> Option<String> {
        match self {
            Self::MySql => Some(format!("KILL QUERY {process_id}")),
            Self::SqlServer => Some(format!("KILL {process_id}")),
            Self::PostgreSql => Some(format!("SELECT pg_cancel_backend({process_id})")),
            Self::CockroachDb | Self::Sqlite => None,
        }
    }

    pub fn count_rows(self, table_ref: &str, where_clause: Option<&str>) -> String {
        let where_sql = where_clause
            .map(|clause| format!(" WHERE {clause}"))
            .unwrap_or_default();
        match self {
            Self::SqlServer => format!("SELECT COUNT_BIG(*) FROM {table_ref}{where_sql}"),
            Self::MySql | Self::PostgreSql | Self::CockroachDb | Self::Sqlite => {
                format!("SELECT COUNT(*) FROM {table_ref}{where_sql}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_identifiers_and_escapes_closing_delimiters_per_dialect() {
        assert_eq!(SqlDialect::MySql.quote_ident("we`ird"), "`we``ird`");
        assert_eq!(SqlDialect::SqlServer.quote_ident("we]ird"), "[we]]ird]");
        assert_eq!(SqlDialect::PostgreSql.quote_ident("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(SqlDialect::Sqlite.quote_ident("orders"), "\"orders\"");
    }

    #[test]
    fn sql_server_qualifies_three_parts_and_defaults_owner_to_dbo() {
        assert_eq!(
            SqlDialect::SqlServer.qualified_table("shop", "sales.orders"),
            "[shop].[sales].[orders]"
        );
        assert_eq!(
            SqlDialect::SqlServer.qualified_table("shop", "orders"),
            "[shop].[dbo].[orders]"
        );
        assert_eq!(SqlDialect::MySql.qualified_table("shop", "orders"), "`shop`.`orders`");
    }

    #[test]
    fn postgres_family_and_sqlite_qualify_schema_dot_table_and_skip_empty_schema() {
        assert_eq!(
            SqlDialect::PostgreSql.qualified_table("public", "orders"),
            "\"public\".\"orders\""
        );
        assert_eq!(SqlDialect::Sqlite.qualified_table("main", "t"), "\"main\".\"t\"");
        assert_eq!(SqlDialect::CockroachDb.qualified_table("", "t"), "\"t\"");
    }

    #[test]
    fn string_literals_escape_backslash_only_for_mysql() {
        let raw = r"C:\path 'x'";
        assert_eq!(SqlDialect::MySql.string_literal(raw), r"'C:\\path ''x'''");
        assert_eq!(SqlDialect::SqlServer.string_literal(raw), r"N'C:\path ''x'''");
        assert_eq!(SqlDialect::PostgreSql.string_literal(raw), r"'C:\path ''x'''");
        assert_eq!(SqlDialect::Sqlite.string_literal(raw), r"'C:\path ''x'''");
    }

    #[test]
    fn single_row_mutations_use_limit_for_mysql_and_top_for_sql_server() {
        assert_eq!(
            SqlDialect::MySql.delete_single_row("`t`", "`id` = 1"),
            "DELETE FROM `t` WHERE `id` = 1 LIMIT 1"
        );
        assert_eq!(
            SqlDialect::SqlServer.update_single_row("[t]", "[a] = 1", "[id] = 2"),
            "UPDATE TOP (1) [t] SET [a] = 1 WHERE [id] = 2"
        );
        assert_eq!(
            SqlDialect::SqlServer.insert_default_values("[t]"),
            "INSERT INTO [t] DEFAULT VALUES"
        );
    }

    #[test]
    fn single_row_mutations_only_use_limit_where_the_engine_supports_it() {
        assert_eq!(
            SqlDialect::PostgreSql.update_single_row("\"t\"", "\"a\" = 1", "\"id\" = 2"),
            "UPDATE \"t\" SET \"a\" = 1 WHERE \"id\" = 2"
        );
        assert_eq!(
            SqlDialect::CockroachDb.delete_single_row("\"t\"", "\"id\" = 2"),
            "DELETE FROM \"t\" WHERE \"id\" = 2 LIMIT 1"
        );
        assert_eq!(
            SqlDialect::Sqlite.delete_single_row("\"t\"", "\"id\" = 2"),
            "DELETE FROM \"t\" WHERE \"id\" = 2"
        );
        assert_eq!(
            SqlDialect::Sqlite.insert_default_values("\"t\""),
            "INSERT INTO \"t\" DEFAULT VALUES"
        );
    }

    #[test]
    fn paging_uses_limit_for_mysql_and_offset_fetch_with_required_order_for_sql_server() {
        assert_eq!(
            SqlDialect::MySql.paged_select("`t`", Some("id > 1"), None, 50, 100),
            "SELECT * FROM `t` WHERE id > 1 LIMIT 50 OFFSET 100"
        );
        assert_eq!(
            SqlDialect::SqlServer.paged_select("[d].[dbo].[t]", None, None, 50, 0),
            "SELECT * FROM [d].[dbo].[t] ORDER BY (SELECT NULL) OFFSET 0 ROWS FETCH NEXT 50 ROWS ONLY"
        );
        assert_eq!(
            SqlDialect::SqlServer.paged_select("[t]", None, Some("[id] DESC"), 10, 20),
            "SELECT * FROM [t] ORDER BY [id] DESC OFFSET 20 ROWS FETCH NEXT 10 ROWS ONLY"
        );
        assert_eq!(
            SqlDialect::PostgreSql.paged_select("\"t\"", None, Some("\"id\" DESC"), 10, 20),
            "SELECT * FROM \"t\" ORDER BY \"id\" DESC LIMIT 10 OFFSET 20"
        );
    }

    #[test]
    fn single_value_lookup_uses_top_only_for_sql_server() {
        assert_eq!(
            SqlDialect::SqlServer.select_single_value("[c]", "[t]", "[id] = 1"),
            "SELECT TOP (1) [c] FROM [t] WHERE [id] = 1"
        );
        assert_eq!(
            SqlDialect::PostgreSql.select_single_value("\"c\"", "\"t\"", "\"id\" = 1"),
            "SELECT \"c\" FROM \"t\" WHERE \"id\" = 1 LIMIT 1"
        );
    }

    #[test]
    fn binary_and_boolean_literals_follow_each_dialect() {
        assert_eq!(SqlDialect::MySql.binary_literal("0AFF"), "0x0AFF");
        assert_eq!(SqlDialect::PostgreSql.binary_literal("0aff"), r"'\x0aff'::bytea");
        assert_eq!(SqlDialect::Sqlite.binary_literal("0AFF"), "X'0AFF'");
        assert_eq!(SqlDialect::Sqlite.boolean_literal(true), "1");
        assert_eq!(SqlDialect::CockroachDb.boolean_literal(false), "FALSE");
    }

    #[test]
    fn kill_statement_matches_each_server_syntax() {
        assert_eq!(SqlDialect::MySql.kill_process(42).as_deref(), Some("KILL QUERY 42"));
        assert_eq!(SqlDialect::SqlServer.kill_process(42).as_deref(), Some("KILL 42"));
        assert_eq!(
            SqlDialect::PostgreSql.kill_process(42).as_deref(),
            Some("SELECT pg_cancel_backend(42)")
        );
        assert_eq!(SqlDialect::Sqlite.kill_process(42), None);
    }

    #[test]
    fn engine_defaults_match_standard_ports_and_socket_support() {
        assert_eq!(DatabaseEngine::default(), DatabaseEngine::MySql);
        assert_eq!(DatabaseEngine::SqlServer.default_port(), 1433);
        assert!(!DatabaseEngine::SqlServer.supports_unix_socket());
        assert_eq!(DatabaseEngine::PostgreSql.default_port(), 5432);
        assert_eq!(DatabaseEngine::CockroachDb.default_port(), 26257);
        assert_eq!(DatabaseEngine::PostgreSql.default_username(), "postgres");
        assert!(DatabaseEngine::PostgreSql.supports_unix_socket());
        assert!(DatabaseEngine::Sqlite.is_file_based());
        assert!(!DatabaseEngine::Sqlite.requires_credentials());
        assert_eq!(DatabaseEngine::CockroachDb.dialect(), SqlDialect::CockroachDb);
    }
}

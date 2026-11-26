use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum DatabaseEngine {
    #[default]
    MySql,
    SqlServer,
}

impl DatabaseEngine {
    pub const ALL: [DatabaseEngine; 2] = [Self::MySql, Self::SqlServer];

    pub fn label(self) -> &'static str {
        match self {
            Self::MySql => "MySQL",
            Self::SqlServer => "SQL Server",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Self::MySql => 3306,
            Self::SqlServer => 1433,
        }
    }

    pub fn dialect(self) -> SqlDialect {
        match self {
            Self::MySql => SqlDialect::MySql,
            Self::SqlServer => SqlDialect::SqlServer,
        }
    }

    pub fn supports_unix_socket(self) -> bool {
        matches!(self, Self::MySql)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SqlDialect {
    #[default]
    MySql,
    SqlServer,
}

impl SqlDialect {
    pub fn quote_ident(self, ident: &str) -> String {
        match self {
            Self::MySql => format!("`{}`", ident.replace('`', "``")),
            Self::SqlServer => format!("[{}]", ident.replace(']', "]]")),
        }
    }

    pub fn qualified_table(self, schema: &str, table: &str) -> String {
        match self {
            Self::MySql => format!("{}.{}", self.quote_ident(schema), self.quote_ident(table)),
            Self::SqlServer => {
                let (owner, object) = table.split_once('.').unwrap_or(("dbo", table));
                format!(
                    "{}.{}.{}",
                    self.quote_ident(schema),
                    self.quote_ident(owner),
                    self.quote_ident(object)
                )
            }
        }
    }

    pub fn string_literal(self, value: &str) -> String {
        match self {
            Self::MySql => format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''")),
            Self::SqlServer => format!("N'{}'", value.replace('\'', "''")),
        }
    }

    pub fn delete_single_row(self, table_ref: &str, predicate: &str) -> String {
        match self {
            Self::MySql => format!("DELETE FROM {table_ref} WHERE {predicate} LIMIT 1"),
            Self::SqlServer => format!("DELETE TOP (1) FROM {table_ref} WHERE {predicate}"),
        }
    }

    pub fn update_single_row(self, table_ref: &str, assignments: &str, predicate: &str) -> String {
        match self {
            Self::MySql => format!("UPDATE {table_ref} SET {assignments} WHERE {predicate} LIMIT 1"),
            Self::SqlServer => format!("UPDATE TOP (1) {table_ref} SET {assignments} WHERE {predicate}"),
        }
    }

    pub fn insert_default_values(self, table_ref: &str) -> String {
        match self {
            Self::MySql => format!("INSERT INTO {table_ref} () VALUES ()"),
            Self::SqlServer => format!("INSERT INTO {table_ref} DEFAULT VALUES"),
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
            Self::MySql => {
                let order_sql = order_by
                    .map(|order| format!(" ORDER BY {order}"))
                    .unwrap_or_default();
                format!("SELECT * FROM {table_ref}{where_sql}{order_sql} LIMIT {limit} OFFSET {offset}")
            }
            Self::SqlServer => {
                let order_sql = order_by.unwrap_or("(SELECT NULL)");
                format!(
                    "SELECT * FROM {table_ref}{where_sql} ORDER BY {order_sql} OFFSET {offset} ROWS FETCH NEXT {limit} ROWS ONLY"
                )
            }
        }
    }

    pub fn count_rows(self, table_ref: &str, where_clause: Option<&str>) -> String {
        let where_sql = where_clause
            .map(|clause| format!(" WHERE {clause}"))
            .unwrap_or_default();
        match self {
            Self::MySql => format!("SELECT COUNT(*) FROM {table_ref}{where_sql}"),
            Self::SqlServer => format!("SELECT COUNT_BIG(*) FROM {table_ref}{where_sql}"),
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
    }

    #[test]
    fn sql_server_qualifies_three_parts_and_defaults_owner_to_dbo() {
        assert_eq!(
            SqlDialect::SqlServer.qualified_table("shop", "sales.orders"),
            "[shop].[sales].[orders]"
        );
        assert_eq!(SqlDialect::SqlServer.qualified_table("shop", "orders"), "[shop].[dbo].[orders]");
        assert_eq!(SqlDialect::MySql.qualified_table("shop", "orders"), "`shop`.`orders`");
    }

    #[test]
    fn string_literals_escape_backslash_only_for_mysql() {
        let raw = r"C:\path 'x'";
        assert_eq!(SqlDialect::MySql.string_literal(raw), r"'C:\\path ''x'''");
        assert_eq!(SqlDialect::SqlServer.string_literal(raw), r"N'C:\path ''x'''");
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
        assert_eq!(SqlDialect::SqlServer.insert_default_values("[t]"), "INSERT INTO [t] DEFAULT VALUES");
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
    }

    #[test]
    fn engine_defaults_match_standard_ports_and_socket_support() {
        assert_eq!(DatabaseEngine::default(), DatabaseEngine::MySql);
        assert_eq!(DatabaseEngine::SqlServer.default_port(), 1433);
        assert!(!DatabaseEngine::SqlServer.supports_unix_socket());
    }
}

use cottondata_core::SqlDialect;
use cottondata_driver::ColumnSummary;

pub(crate) fn render_create_table(
    owner: &str,
    object: &str,
    columns: &[ColumnSummary],
    primary_key_columns: &[String],
) -> String {
    let dialect = SqlDialect::SqlServer;
    let mut lines: Vec<String> = columns
        .iter()
        .map(|column| {
            let mut line = format!("    {} {}", dialect.quote_ident(&column.name), column.column_type);
            if column.is_auto_increment {
                line.push_str(" IDENTITY");
            }
            line.push_str(if column.is_nullable { " NULL" } else { " NOT NULL" });
            if let Some(default_expr) = &column.default_expr {
                line.push_str(" DEFAULT ");
                line.push_str(default_expr);
            }
            line
        })
        .collect();
    if !primary_key_columns.is_empty() {
        let key_list = primary_key_columns
            .iter()
            .map(|column| dialect.quote_ident(column))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("    PRIMARY KEY ({key_list})"));
    }
    format!(
        "CREATE TABLE {}.{} (\n{}\n);",
        dialect.quote_ident(owner),
        dialect.quote_ident(object),
        lines.join(",\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, column_type: &str, is_nullable: bool, is_auto_increment: bool) -> ColumnSummary {
        ColumnSummary {
            name: name.into(),
            data_type: column_type.into(),
            column_type: column_type.into(),
            is_primary_key: false,
            is_nullable,
            is_auto_increment,
            default_expr: None,
            comment: None,
        }
    }

    #[test]
    fn renders_create_table_with_identity_nullability_defaults_and_primary_key() {
        let mut status = column("status", "nvarchar(20)", false, false);
        status.default_expr = Some("(N'active')".into());
        let ddl = render_create_table(
            "sales",
            "orders",
            &[
                column("id", "int", false, true),
                status,
                column("note", "nvarchar(max)", true, false),
            ],
            &["id".into()],
        );
        assert_eq!(
            ddl,
            "CREATE TABLE [sales].[orders] (\n    [id] int IDENTITY NOT NULL,\n    [status] nvarchar(20) NOT NULL DEFAULT (N'active'),\n    [note] nvarchar(max) NULL,\n    PRIMARY KEY ([id])\n);"
        );
    }
}

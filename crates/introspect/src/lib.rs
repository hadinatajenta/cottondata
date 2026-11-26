use cottondata_mysql::{MySqlError, MySqlSession};
use cottondata_sql::SchemaTableHint;
use lasso::{Rodeo, Spur};
use mysql::prelude::Queryable;
use std::collections::BTreeMap;

pub const SYSTEM_SCHEMAS: &[&str] = &[
    "information_schema",
    "mysql",
    "performance_schema",
    "sys",
];

pub fn is_system_schema(name: &str) -> bool {
    SYSTEM_SCHEMAS.iter().any(|s| s.eq_ignore_ascii_case(name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableKind {
    BaseTable,
    View,
}

#[derive(Debug, Clone)]
pub struct ColumnBrief {
    pub name: Spur,
    pub data_type: Spur,
    pub column_type: Spur,
    pub is_primary_key: bool,
    pub is_nullable: bool,
    pub is_auto_increment: bool,
    pub default_expr: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone)]
pub struct IndexInfo {
    pub name: String,
    pub non_unique: bool,
    pub index_type: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ForeignKeyInfo {
    pub constraint_name: String,
    pub column_name: String,
    pub ref_schema: String,
    pub ref_table: String,
    pub ref_column: String,
    pub update_rule: String,
    pub delete_rule: String,
}

#[derive(Debug, Clone)]
pub struct TableDetail {
    pub ddl: String,
    pub engine: Option<String>,
    pub collation: Option<String>,
    pub row_estimate: Option<u64>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
}

#[derive(Debug, Clone)]
pub struct TableEntry {
    pub name: Spur,
    pub kind: TableKind,
    pub columns: Vec<ColumnBrief>,
    pub detail: Option<TableDetail>,
}

#[derive(Debug, Clone)]
pub struct SchemaEntry {
    pub name: Spur,
    pub default_collation: Option<Spur>,
    pub is_system: bool,
    pub l1_loaded: bool,
    pub tables: Vec<TableEntry>,
    pub routines: Vec<(Spur, Spur)>,
}

#[derive(Debug, Default)]
pub struct SchemaCatalog {
    interner: Rodeo,
    schemas: Vec<SchemaEntry>,
}

impl SchemaCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resolve(&self, spur: Spur) -> &str {
        self.interner.resolve(&spur)
    }

    pub fn snapshot(&self) -> Self {
        let mut copy = Self::new();
        for s in &self.schemas {
            let name = copy.interner.get_or_intern(self.interner.resolve(&s.name));
            let default_collation = s
                .default_collation
                .map(|c| copy.interner.get_or_intern(self.interner.resolve(&c)));
            let tables = s
                .tables
                .iter()
                .map(|t| TableEntry {
                    name: copy.interner.get_or_intern(self.interner.resolve(&t.name)),
                    kind: t.kind,
                    columns: t
                        .columns
                        .iter()
                        .map(|c| ColumnBrief {
                            name: copy.interner.get_or_intern(self.interner.resolve(&c.name)),
                            data_type: copy.interner.get_or_intern(self.interner.resolve(&c.data_type)),
                            column_type: copy.interner.get_or_intern(self.interner.resolve(&c.column_type)),
                            is_primary_key: c.is_primary_key,
                            is_nullable: c.is_nullable,
                            is_auto_increment: c.is_auto_increment,
                            default_expr: c.default_expr.clone(),
                            comment: c.comment.clone(),
                        })
                        .collect(),
                    detail: t.detail.clone(),
                })
                .collect();
            let routines = s
                .routines
                .iter()
                .map(|(n, k)| {
                    (
                        copy.interner.get_or_intern(self.interner.resolve(n)),
                        copy.interner.get_or_intern(self.interner.resolve(k)),
                    )
                })
                .collect();
            copy.schemas.push(SchemaEntry {
                name,
                default_collation,
                is_system: s.is_system,
                l1_loaded: s.l1_loaded,
                tables,
                routines,
            });
        }
        copy
    }

    pub fn schemas(&self) -> &[SchemaEntry] {
        &self.schemas
    }

    pub fn approx_bytes(&self) -> usize {
        let interner_bytes = self.interner.current_memory_usage();
        let meta_bytes: usize = self
            .schemas
            .iter()
            .map(|s| {
                std::mem::size_of::<SchemaEntry>()
                    + s.tables
                        .iter()
                        .map(|t| {
                            std::mem::size_of::<TableEntry>()
                                + t.columns.len() * std::mem::size_of::<ColumnBrief>()
                                + t.detail.as_ref().map(|d| d.ddl.len() + 256).unwrap_or(0)
                        })
                        .sum::<usize>()
            })
            .sum();
        interner_bytes + meta_bytes
    }

    pub fn introspect_l0(&mut self, session: &mut MySqlSession) -> Result<(), MySqlError> {
        let rows: Vec<(String, Option<String>)> = session
            .conn_mut()
            .query(
                "SELECT schema_name, default_collation_name \
                 FROM information_schema.schemata \
                 ORDER BY schema_name",
            )
            .map_err(MySqlError::from_mysql)?;

        self.schemas.clear();
        for (name, collation) in rows {
            let sys = is_system_schema(&name);
            let name_spur = self.interner.get_or_intern(&name);
            let col_spur = collation.as_deref().map(|c| self.interner.get_or_intern(c));
            self.schemas.push(SchemaEntry {
                name: name_spur,
                default_collation: col_spur,
                is_system: sys,
                l1_loaded: false,
                tables: Vec::new(),
                routines: Vec::new(),
            });
        }
        Ok(())
    }

    pub fn introspect_schema_l1(
        &mut self,
        session: &mut MySqlSession,
        schema_name: &str,
    ) -> Result<(), MySqlError> {
        let table_rows: Vec<(String, String)> = session
            .conn_mut()
            .exec(
                "SELECT table_name, table_type \
                 FROM information_schema.tables \
                 WHERE table_schema = ? \
                 ORDER BY table_name",
                (schema_name,),
            )
            .map_err(MySqlError::from_mysql)?;

        type ColumnRow = (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        );
        let col_rows: Vec<ColumnRow> =
            session
                .conn_mut()
                .exec(
                    "SELECT table_name, column_name, data_type, column_type, \
                            column_key, is_nullable, extra, column_default, column_comment \
                     FROM information_schema.columns \
                     WHERE table_schema = ? \
                     ORDER BY table_name, ordinal_position",
                    (schema_name,),
                )
                .map_err(MySqlError::from_mysql)?;

        let routine_rows: Vec<(String, String)> = session
            .conn_mut()
            .exec(
                "SELECT routine_name, routine_type \
                 FROM information_schema.routines \
                 WHERE routine_schema = ? \
                 ORDER BY routine_name",
                (schema_name,),
            )
            .unwrap_or_default();

        let mut cols_by_table: BTreeMap<String, Vec<ColumnBrief>> = BTreeMap::new();
        for (tbl, col, dtype, ctype, ckey, nullable, extra, def, comment) in col_rows {
            let entry = ColumnBrief {
                name: self.interner.get_or_intern(&col),
                data_type: self.interner.get_or_intern(&dtype),
                column_type: self.interner.get_or_intern(&ctype),
                is_primary_key: ckey.eq_ignore_ascii_case("PRI"),
                is_nullable: nullable.eq_ignore_ascii_case("YES"),
                is_auto_increment: extra.to_ascii_lowercase().contains("auto_increment"),
                default_expr: def,
                comment: comment.filter(|c| !c.is_empty()),
            };
            cols_by_table.entry(tbl).or_default().push(entry);
        }

        let mut tables = Vec::with_capacity(table_rows.len());
        for (tname, ttype) in table_rows {
            let cols = cols_by_table.remove(&tname).unwrap_or_default();
            let kind = if ttype.eq_ignore_ascii_case("VIEW") {
                TableKind::View
            } else {
                TableKind::BaseTable
            };
            tables.push(TableEntry {
                name: self.interner.get_or_intern(&tname),
                kind,
                columns: cols,
                detail: None,
            });
        }

        let routines: Vec<(Spur, Spur)> = routine_rows
            .into_iter()
            .map(|(n, k)| (self.interner.get_or_intern(&n), self.interner.get_or_intern(&k)))
            .collect();

        if let Some(schema) = self
            .schemas
            .iter_mut()
            .find(|s| self.interner.resolve(&s.name).eq_ignore_ascii_case(schema_name))
        {
            schema.tables = tables;
            schema.routines = routines;
            schema.l1_loaded = true;
        }
        Ok(())
    }

    pub fn introspect_table_l2(
        &mut self,
        session: &mut MySqlSession,
        schema_name: &str,
        table_name: &str,
    ) -> Result<TableDetail, MySqlError> {
        let fq = format!(
            "`{}`.`{}`",
            schema_name.replace('`', "``"),
            table_name.replace('`', "``")
        );
        let ddl_row: Option<(String, String)> = session
            .conn_mut()
            .query_first(format!("SHOW CREATE TABLE {fq}"))
            .map_err(MySqlError::from_mysql)?;
        let ddl = ddl_row.map(|(_, d)| d).unwrap_or_default();

        let meta_row: Option<(Option<String>, Option<String>, Option<u64>)> = session
            .conn_mut()
            .exec_first(
                "SELECT engine, table_collation, table_rows \
                 FROM information_schema.tables \
                 WHERE table_schema = ? AND table_name = ?",
                (schema_name, table_name),
            )
            .unwrap_or(None);
        let (engine, collation, row_estimate) = meta_row.unwrap_or((None, None, None));

        let idx_rows: Vec<(String, u8, String, Option<String>)> = session
            .conn_mut()
            .exec(
                "SELECT index_name, non_unique, index_type, column_name \
                 FROM information_schema.statistics \
                 WHERE table_schema = ? AND table_name = ? \
                 ORDER BY index_name, seq_in_index",
                (schema_name, table_name),
            )
            .unwrap_or_default();

        let mut idx_map: BTreeMap<String, IndexInfo> = BTreeMap::new();
        for (name, non_unique, index_type, col) in idx_rows {
            let entry = idx_map.entry(name.clone()).or_insert_with(|| IndexInfo {
                name,
                non_unique: non_unique != 0,
                index_type,
                columns: Vec::new(),
            });
            if let Some(c) = col {
                entry.columns.push(c);
            }
        }

        let fk_rows: Vec<(String, String, String, String, String, String, String)> = session
            .conn_mut()
            .exec(
                "SELECT k.constraint_name, k.column_name, k.referenced_table_schema, \
                        k.referenced_table_name, k.referenced_column_name, \
                        r.update_rule, r.delete_rule \
                 FROM information_schema.key_column_usage k \
                 JOIN information_schema.referential_constraints r \
                   ON k.constraint_schema = r.constraint_schema \
                  AND k.constraint_name = r.constraint_name \
                 WHERE k.table_schema = ? AND k.table_name = ? \
                   AND k.referenced_column_name IS NOT NULL \
                 ORDER BY k.constraint_name, k.ordinal_position",
                (schema_name, table_name),
            )
            .unwrap_or_default();

        let foreign_keys: Vec<ForeignKeyInfo> = fk_rows
            .into_iter()
            .map(|(cn, col, rs, rt, rc, ur, dr)| ForeignKeyInfo {
                constraint_name: cn,
                column_name: col,
                ref_schema: rs,
                ref_table: rt,
                ref_column: rc,
                update_rule: ur,
                delete_rule: dr,
            })
            .collect();

        let detail = TableDetail {
            ddl,
            engine,
            collation,
            row_estimate,
            indexes: idx_map.into_values().collect(),
            foreign_keys,
        };

        if let Some(schema) = self
            .schemas
            .iter_mut()
            .find(|s| self.interner.resolve(&s.name).eq_ignore_ascii_case(schema_name))
        {
            if let Some(table) = schema
                .tables
                .iter_mut()
                .find(|t| self.interner.resolve(&t.name).eq_ignore_ascii_case(table_name))
            {
                table.detail = Some(detail.clone());
            }
        }

        Ok(detail)
    }

    pub fn primary_keys_for(&self, schema_name: &str, table_name: &str) -> Vec<String> {
        let Some(schema) = self
            .schemas
            .iter()
            .find(|s| self.interner.resolve(&s.name).eq_ignore_ascii_case(schema_name))
        else {
            return Vec::new();
        };
        let Some(table) = schema
            .tables
            .iter()
            .find(|t| self.interner.resolve(&t.name).eq_ignore_ascii_case(table_name))
        else {
            return Vec::new();
        };
        table
            .columns
            .iter()
            .filter(|c| c.is_primary_key)
            .map(|c| self.interner.resolve(&c.name).to_string())
            .collect()
    }

    pub fn completion_hints(&self, active_schema: Option<&str>) -> Vec<SchemaTableHint> {
        let mut hints = Vec::new();
        for schema in &self.schemas {
            let sname = self.interner.resolve(&schema.name);
            if let Some(target) = active_schema {
                if !sname.eq_ignore_ascii_case(target) {
                    continue;
                }
            } else if schema.is_system {
                continue;
            }
            for table in &schema.tables {
                hints.push(SchemaTableHint {
                    name: self.interner.resolve(&table.name).to_string(),
                    columns: table
                        .columns
                        .iter()
                        .map(|c| {
                            (
                                self.interner.resolve(&c.name).to_string(),
                                self.interner.resolve(&c.column_type).to_string(),
                            )
                        })
                        .collect(),
                });
            }
        }
        hints
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_standard_mysql_system_schemas() {
        assert!(is_system_schema("information_schema"));
        assert!(is_system_schema("performance_schema"));
        assert!(is_system_schema("mysql"));
        assert!(is_system_schema("sys"));
        assert!(!is_system_schema("production_app"));
    }
}

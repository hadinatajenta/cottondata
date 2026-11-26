use cottondata_driver::{DatabaseSession, DbError, SchemaObjects, SchemaSummary};
pub use cottondata_driver::{ForeignKeyInfo, IndexInfo, TableDetail, TableKind};
use cottondata_sql::SchemaTableHint;
use lasso::{Rodeo, Spur};

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

    pub fn introspect_l0(&mut self, session: &mut dyn DatabaseSession) -> Result<(), DbError> {
        let schemas = session.list_schemas()?;
        self.apply_schemas(schemas);
        Ok(())
    }

    pub fn introspect_schema_l1(&mut self, session: &mut dyn DatabaseSession, schema_name: &str) -> Result<(), DbError> {
        let objects = session.load_schema_objects(schema_name)?;
        self.apply_schema_objects(schema_name, objects);
        Ok(())
    }

    pub fn introspect_table_l2(
        &mut self,
        session: &mut dyn DatabaseSession,
        schema_name: &str,
        table_name: &str,
    ) -> Result<TableDetail, DbError> {
        let detail = session.load_table_detail(schema_name, table_name)?;
        let Some(schema_index) = self.find_schema_index(schema_name) else {
            return Ok(detail);
        };
        let interner = &self.interner;
        if let Some(table) = self.schemas[schema_index]
            .tables
            .iter_mut()
            .find(|table| interner.resolve(&table.name).eq_ignore_ascii_case(table_name))
        {
            table.detail = Some(detail.clone());
        }
        Ok(detail)
    }

    pub fn apply_schemas(&mut self, schemas: Vec<SchemaSummary>) {
        self.schemas.clear();
        for summary in schemas {
            let name = self.interner.get_or_intern(&summary.name);
            let default_collation = summary
                .default_collation
                .as_deref()
                .map(|collation| self.interner.get_or_intern(collation));
            self.schemas.push(SchemaEntry {
                name,
                default_collation,
                is_system: summary.is_system,
                l1_loaded: false,
                tables: Vec::new(),
                routines: Vec::new(),
            });
        }
    }

    pub fn apply_schema_objects(&mut self, schema_name: &str, objects: SchemaObjects) {
        let tables: Vec<TableEntry> = objects
            .tables
            .into_iter()
            .map(|table| TableEntry {
                name: self.interner.get_or_intern(&table.name),
                kind: table.kind,
                columns: table
                    .columns
                    .into_iter()
                    .map(|column| ColumnBrief {
                        name: self.interner.get_or_intern(&column.name),
                        data_type: self.interner.get_or_intern(&column.data_type),
                        column_type: self.interner.get_or_intern(&column.column_type),
                        is_primary_key: column.is_primary_key,
                        is_nullable: column.is_nullable,
                        is_auto_increment: column.is_auto_increment,
                        default_expr: column.default_expr,
                        comment: column.comment,
                    })
                    .collect(),
                detail: None,
            })
            .collect();
        let routines: Vec<(Spur, Spur)> = objects
            .routines
            .into_iter()
            .map(|routine| {
                (
                    self.interner.get_or_intern(&routine.name),
                    self.interner.get_or_intern(&routine.kind),
                )
            })
            .collect();
        if let Some(schema_index) = self.find_schema_index(schema_name) {
            let schema = &mut self.schemas[schema_index];
            schema.tables = tables;
            schema.routines = routines;
            schema.l1_loaded = true;
        }
    }

    fn find_schema_index(&self, schema_name: &str) -> Option<usize> {
        self.schemas
            .iter()
            .position(|schema| self.interner.resolve(&schema.name).eq_ignore_ascii_case(schema_name))
    }

    pub fn primary_keys_for(&self, schema_name: &str, table_name: &str) -> Vec<String> {
        let Some(table) = self.find_table(schema_name, table_name) else {
            return Vec::new();
        };
        table
            .columns
            .iter()
            .filter(|c| c.is_primary_key)
            .map(|c| self.interner.resolve(&c.name).to_string())
            .collect()
    }

    pub fn table_kind(&self, schema_name: &str, table_name: &str) -> Option<TableKind> {
        self.find_table(schema_name, table_name).map(|table| table.kind)
    }

    pub fn unique_key_columns_for(&self, schema_name: &str, table_name: &str) -> Vec<String> {
        let Some(table) = self.find_table(schema_name, table_name) else {
            return Vec::new();
        };
        let Some(detail) = &table.detail else {
            return Vec::new();
        };
        for index in &detail.indexes {
            if index.non_unique || index.columns.is_empty() {
                continue;
            }
            let all_not_null = index.columns.iter().all(|col_name| {
                table.columns.iter().any(|c| {
                    self.interner.resolve(&c.name).eq_ignore_ascii_case(col_name) && !c.is_nullable
                })
            });
            if all_not_null {
                return index.columns.clone();
            }
        }
        Vec::new()
    }

    fn find_table(&self, schema_name: &str, table_name: &str) -> Option<&TableEntry> {
        let schema = self
            .schemas
            .iter()
            .find(|s| self.interner.resolve(&s.name).eq_ignore_ascii_case(schema_name))?;
        schema
            .tables
            .iter()
            .find(|t| self.interner.resolve(&t.name).eq_ignore_ascii_case(table_name))
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
    use cottondata_driver::{ColumnSummary, TableSummary};

    fn column(name: &str, is_primary_key: bool) -> ColumnSummary {
        ColumnSummary {
            name: name.into(),
            data_type: "int".into(),
            column_type: "int(11)".into(),
            is_primary_key,
            is_nullable: false,
            is_auto_increment: false,
            default_expr: None,
            comment: None,
        }
    }

    #[test]
    fn applies_schema_list_then_lazily_loaded_objects() {
        let mut catalog = SchemaCatalog::new();
        catalog.apply_schemas(vec![
            SchemaSummary {
                name: "app".into(),
                default_collation: None,
                is_system: false,
            },
            SchemaSummary {
                name: "mysql".into(),
                default_collation: None,
                is_system: true,
            },
        ]);
        assert_eq!(catalog.schemas().len(), 2);
        assert!(!catalog.schemas()[0].l1_loaded);

        catalog.apply_schema_objects(
            "APP",
            SchemaObjects {
                tables: vec![TableSummary {
                    name: "users".into(),
                    kind: TableKind::BaseTable,
                    columns: vec![column("id", true), column("org_id", false)],
                }],
                routines: Vec::new(),
            },
        );
        assert!(catalog.schemas()[0].l1_loaded);
        assert_eq!(catalog.primary_keys_for("app", "users"), vec!["id".to_string()]);
        assert_eq!(catalog.completion_hints(None).len(), 1);
    }

    #[test]
    fn objects_for_unknown_schema_are_ignored() {
        let mut catalog = SchemaCatalog::new();
        catalog.apply_schema_objects("missing", SchemaObjects::default());
        assert!(catalog.schemas().is_empty());
    }
}

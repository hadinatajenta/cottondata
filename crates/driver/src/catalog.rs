#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableKind {
    BaseTable,
    View,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSummary {
    pub name: String,
    pub default_collation: Option<String>,
    pub is_system: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSummary {
    pub name: String,
    pub data_type: String,
    pub column_type: String,
    pub is_primary_key: bool,
    pub is_nullable: bool,
    pub is_auto_increment: bool,
    pub default_expr: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSummary {
    pub name: String,
    pub kind: TableKind,
    pub columns: Vec<ColumnSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutineSummary {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaObjects {
    pub tables: Vec<TableSummary>,
    pub routines: Vec<RoutineSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexInfo {
    pub name: String,
    pub non_unique: bool,
    pub index_type: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKeyInfo {
    pub constraint_name: String,
    pub column_name: String,
    pub ref_schema: String,
    pub ref_table: String,
    pub ref_column: String,
    pub update_rule: String,
    pub delete_rule: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundForeignKeyInfo {
    pub constraint_name: String,
    pub source_schema: String,
    pub source_table: String,
    pub source_column: String,
    pub target_column: String,
    pub update_rule: String,
    pub delete_rule: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableDetail {
    pub ddl: String,
    pub engine: Option<String>,
    pub collation: Option<String>,
    pub row_estimate: Option<u64>,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKeyInfo>,
    pub inbound_foreign_keys: Vec<InboundForeignKeyInfo>,
}

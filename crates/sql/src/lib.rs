pub mod complete;
pub mod ddl_builder;
pub mod edit;
pub mod fold;
pub mod format;
pub mod lexer;
pub mod lint;
pub mod paging;
pub mod params;
pub mod safety;

pub use complete::{CompletionItem, CompletionKind, CompletionProvider, SchemaTableHint};
pub use ddl_builder::{
    build_alter_table_statements, ColumnDraft, ForeignKeyDraft, IndexDraft, TableStructureEdit,
};
pub use edit::{
    duplicate_lines, find_matches, move_lines_down, move_lines_up, replace_all_matches,
    replace_match, toggle_line_comment, EditResult, SearchOptions,
};
pub use fold::{
    compute_fold_regions, compute_line_spans, gutter_fold_regions, innermost_region_at_line,
    line_index_for_byte, FoldKind, FoldRegion, LineByteSpan,
};
pub use format::format_mysql;
pub use lexer::{split_statements, statement_at_offset, tokenize, SqlStatement, Token, TokenKind};
pub use lint::{
    lint_sql, resolve_server_error_diagnostic, SqlDiagSeverity, SqlDiagSource, SqlDiagnostic,
    SqlQuickFix,
};
pub use paging::{
    build_paginated_query, build_sorted_query, can_auto_paginate, has_explicit_row_limit,
    is_pageable_read_query, strip_trailing_terminator,
};
pub use params::{
    bind_query_params, bind_statements_params, extract_params_for_statements,
    extract_query_params, format_param_literal, query_param_fingerprint, ParamBinding,
    ParamValueKind, QueryParamSpec,
};
pub use safety::{analyze_statement, is_explainable_statement, StatementSafety, WriteClass};

use crate::lexer::{tokenize, TokenKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteClass {
    ReadOnly,
    SessionControl,
    DataMutation,
    SchemaMutation,
    Destructive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementSafety {
    pub class: WriteClass,
    pub warning: Option<String>,
    pub affected_table: Option<String>,
}

impl StatementSafety {
    pub fn allowed_in_read_only(&self) -> bool {
        matches!(self.class, WriteClass::ReadOnly | WriteClass::SessionControl)
    }
}

pub fn analyze_statement(sql: &str) -> StatementSafety {
    let tokens: Vec<_> = tokenize(sql)
        .into_iter()
        .filter(|t| !matches!(t.kind, TokenKind::Whitespace | TokenKind::Comment))
        .collect();

    if tokens.is_empty() {
        return StatementSafety {
            class: WriteClass::ReadOnly,
            warning: None,
            affected_table: None,
        };
    }

    let first = tokens[0].text.to_ascii_uppercase();
    let second = tokens.get(1).map(|t| t.text.to_ascii_uppercase());
    let has_where = tokens.iter().any(|t| t.text.eq_ignore_ascii_case("WHERE"));
    let has_limit = tokens.iter().any(|t| t.text.eq_ignore_ascii_case("LIMIT"));

    match first.as_str() {
        "SELECT" | "SHOW" | "DESCRIBE" | "DESC" | "EXPLAIN" | "HELP" => StatementSafety {
            class: WriteClass::ReadOnly,
            warning: None,
            affected_table: None,
        },
        "WITH" => {
            let has_mutation = tokens.iter().any(|t| {
                matches!(
                    t.text.to_ascii_uppercase().as_str(),
                    "INSERT" | "UPDATE" | "DELETE" | "REPLACE"
                )
            });
            StatementSafety {
                class: if has_mutation {
                    WriteClass::DataMutation
                } else {
                    WriteClass::ReadOnly
                },
                warning: None,
                affected_table: None,
            }
        }
        "USE" | "SET" | "START" | "BEGIN" | "COMMIT" | "ROLLBACK" | "SAVEPOINT" => StatementSafety {
            class: WriteClass::SessionControl,
            warning: None,
            affected_table: None,
        },
        "UPDATE" => {
            let table = tokens.get(1).map(|t| strip_backticks(t.text));
            let warning = if !has_where {
                Some("UPDATE without WHERE clause will modify every row in the table.".into())
            } else {
                None
            };
            StatementSafety {
                class: if warning.is_some() {
                    WriteClass::Destructive
                } else {
                    WriteClass::DataMutation
                },
                warning,
                affected_table: table,
            }
        }
        "DELETE" => {
            let table = tokens
                .iter()
                .position(|t| t.text.eq_ignore_ascii_case("FROM"))
                .and_then(|idx| tokens.get(idx + 1))
                .map(|t| strip_backticks(t.text));
            let warning = if !has_where && !has_limit {
                Some("DELETE without WHERE clause will remove every row in the table.".into())
            } else {
                None
            };
            StatementSafety {
                class: if warning.is_some() {
                    WriteClass::Destructive
                } else {
                    WriteClass::DataMutation
                },
                warning,
                affected_table: table,
            }
        }
        "DROP" | "TRUNCATE" => {
            let target = tokens
                .iter()
                .skip(1)
                .find(|t| {
                    !matches!(
                        t.text.to_ascii_uppercase().as_str(),
                        "TABLE" | "DATABASE" | "SCHEMA" | "VIEW" | "IF" | "EXISTS" | "TEMPORARY"
                    )
                })
                .map(|t| strip_backticks(t.text));
            StatementSafety {
                class: WriteClass::Destructive,
                warning: Some(format!("Destructive {first} statement detected.")),
                affected_table: target,
            }
        }
        "CREATE" | "ALTER" | "RENAME" => {
            let target = tokens
                .iter()
                .skip(2)
                .find(|t| !matches!(t.text.to_ascii_uppercase().as_str(), "IF" | "NOT" | "EXISTS"))
                .map(|t| strip_backticks(t.text));
            let warning = if first == "ALTER"
                && tokens.iter().any(|t| t.text.eq_ignore_ascii_case("DROP"))
            {
                Some("ALTER TABLE with DROP removes columns or indexes permanently.".into())
            } else {
                None
            };
            StatementSafety {
                class: WriteClass::SchemaMutation,
                warning,
                affected_table: target,
            }
        }
        "INSERT" | "REPLACE" | "LOAD" | "CALL" => {
            let table = if first == "INSERT" || first == "REPLACE" {
                tokens
                    .iter()
                    .position(|t| t.text.eq_ignore_ascii_case("INTO"))
                    .and_then(|idx| tokens.get(idx + 1))
                    .or_else(|| tokens.get(1))
                    .map(|t| strip_backticks(t.text))
            } else {
                None
            };
            StatementSafety {
                class: WriteClass::DataMutation,
                warning: None,
                affected_table: table,
            }
        }
        "KILL" => StatementSafety {
            class: if second.as_deref() == Some("QUERY") {
                WriteClass::SessionControl
            } else {
                WriteClass::Destructive
            },
            warning: None,
            affected_table: None,
        },
        _ => StatementSafety {
            class: WriteClass::DataMutation,
            warning: None,
            affected_table: None,
        },
    }
}

fn strip_backticks(s: &str) -> String {
    s.trim_matches('`').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_unqualified_update_and_delete() {
        let upd = analyze_statement("UPDATE users SET active = 0");
        assert_eq!(upd.class, WriteClass::Destructive);
        assert!(upd.warning.is_some());
        assert_eq!(upd.affected_table.as_deref(), Some("users"));

        let safe_upd = analyze_statement("UPDATE users SET active = 0 WHERE id = 42");
        assert_eq!(safe_upd.class, WriteClass::DataMutation);
        assert!(safe_upd.warning.is_none());

        let sel = analyze_statement("SELECT * FROM users");
        assert!(sel.allowed_in_read_only());
        assert!(!safe_upd.allowed_in_read_only());
    }
}

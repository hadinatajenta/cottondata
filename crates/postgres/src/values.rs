use tokio_postgres::types::{Kind, Type};

pub(crate) fn type_label(column_type: &Type) -> String {
    match column_type.kind() {
        Kind::Array(element) => format!("{}[]", element.name()),
        _ => column_type.name().to_string(),
    }
}

pub(crate) fn is_numeric_type(column_type: &Type) -> bool {
    [Type::INT2, Type::INT4, Type::INT8, Type::FLOAT4, Type::FLOAT8, Type::NUMERIC, Type::OID].contains(column_type)
}

pub(crate) fn is_binary_type(column_type: &Type) -> bool {
    *column_type == Type::BYTEA
}

pub(crate) fn display_text(column_type: Option<&Type>, raw: &str) -> String {
    match (column_type, raw) {
        (Some(kind), "t") if *kind == Type::BOOL => "true".to_string(),
        (Some(kind), "f") if *kind == Type::BOOL => "false".to_string(),
        _ => raw.to_string(),
    }
}

pub(crate) fn is_true_text(raw: Option<&str>) -> bool {
    matches!(
        raw.map(str::trim).map(str::to_ascii_lowercase).as_deref(),
        Some("t" | "true" | "yes" | "1" | "on")
    )
}

pub(crate) fn referential_action(code: &str) -> &'static str {
    match code {
        "r" => "RESTRICT",
        "c" => "CASCADE",
        "n" => "SET NULL",
        "d" => "SET DEFAULT",
        _ => "NO ACTION",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_arrays_with_brackets_and_keeps_builtin_names() {
        assert_eq!(type_label(&Type::INT4_ARRAY), "int4[]");
        assert_eq!(type_label(&Type::TIMESTAMPTZ), "timestamptz");
    }

    #[test]
    fn classifies_numeric_and_binary_types() {
        assert!(is_numeric_type(&Type::NUMERIC));
        assert!(is_numeric_type(&Type::INT8));
        assert!(!is_numeric_type(&Type::TEXT));
        assert!(is_binary_type(&Type::BYTEA));
    }

    #[test]
    fn booleans_display_as_words_and_other_text_is_untouched() {
        assert_eq!(display_text(Some(&Type::BOOL), "t"), "true");
        assert_eq!(display_text(Some(&Type::BOOL), "f"), "false");
        assert_eq!(display_text(Some(&Type::TEXT), "t"), "t");
        assert_eq!(display_text(None, "{1,2}"), "{1,2}");
        assert!(is_true_text(Some("t")));
        assert!(!is_true_text(Some("f")));
        assert!(!is_true_text(None));
    }

    #[test]
    fn referential_action_codes_map_to_sql_keywords() {
        assert_eq!(referential_action("c"), "CASCADE");
        assert_eq!(referential_action("a"), "NO ACTION");
        assert_eq!(referential_action("n"), "SET NULL");
    }
}

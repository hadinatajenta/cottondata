use rusqlite::types::ValueRef;
use std::fmt::Write;

pub(crate) fn value_text(value: ValueRef<'_>) -> Option<String> {
    match value {
        ValueRef::Null => None,
        ValueRef::Integer(number) => Some(number.to_string()),
        ValueRef::Real(number) => Some(format_real(number)),
        ValueRef::Text(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Blob(bytes) => Some(hex_literal(bytes)),
    }
}

fn format_real(number: f64) -> String {
    if number.is_finite() && number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{number:.1}")
    } else {
        number.to_string()
    }
}

fn hex_literal(bytes: &[u8]) -> String {
    let mut literal = String::with_capacity(2 + bytes.len() * 2);
    literal.push_str("0x");
    for byte in bytes {
        let _ = write!(literal, "{byte:02X}");
    }
    literal
}

pub(crate) fn is_numeric_declared_type(declared_type: &str) -> bool {
    let upper = declared_type.to_ascii_uppercase();
    ["INT", "REAL", "FLOA", "DOUB", "NUM", "DEC"]
        .iter()
        .any(|marker| upper.contains(marker))
}

pub(crate) fn is_binary_declared_type(declared_type: &str) -> bool {
    declared_type.to_ascii_uppercase().contains("BLOB")
}

pub(crate) fn type_label(declared_type: Option<&str>) -> String {
    declared_type
        .map(str::trim)
        .filter(|declared| !declared.is_empty())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "any".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_storage_classes_to_display_text() {
        assert_eq!(value_text(ValueRef::Null), None);
        assert_eq!(value_text(ValueRef::Integer(42)), Some("42".into()));
        assert_eq!(value_text(ValueRef::Real(2.5)), Some("2.5".into()));
        assert_eq!(value_text(ValueRef::Real(3.0)), Some("3.0".into()));
        assert_eq!(value_text(ValueRef::Text("héllo".as_bytes())), Some("héllo".into()));
        assert_eq!(value_text(ValueRef::Blob(&[0x0A, 0xFF])), Some("0x0AFF".into()));
    }

    #[test]
    fn classifies_declared_types_by_sqlite_affinity_rules() {
        assert!(is_numeric_declared_type("INTEGER"));
        assert!(is_numeric_declared_type("decimal(10,2)"));
        assert!(is_numeric_declared_type("DOUBLE PRECISION"));
        assert!(!is_numeric_declared_type("TEXT"));
        assert!(is_binary_declared_type("blob"));
        assert_eq!(type_label(None), "any");
        assert_eq!(type_label(Some("VARCHAR(20)")), "varchar(20)");
    }
}

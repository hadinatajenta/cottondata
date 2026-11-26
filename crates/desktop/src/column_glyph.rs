use crate::ui_kit::icon;

pub fn column_type_glyph(type_name: &str) -> &'static str {
    let normalized = type_name.trim().to_ascii_lowercase();
    let base = normalized
        .split(|character: char| character == '(' || character.is_whitespace())
        .next()
        .unwrap_or_default();
    match base {
        "tinyint" if normalized.starts_with("tinyint(1)") => icon::TOGGLE_LEFT,
        "bool" | "boolean" => icon::TOGGLE_LEFT,
        "tinyint" | "smallint" | "mediumint" | "int" | "integer" | "bigint" | "decimal" | "numeric" | "float"
        | "double" | "real" | "year" => icon::HASH,
        "date" => icon::CALENDAR_BLANK,
        "datetime" | "timestamp" | "time" => icon::CLOCK,
        "json" => icon::BRACKETS_CURLY,
        "binary" | "varbinary" | "blob" | "tinyblob" | "mediumblob" | "longblob" | "bit" => icon::BINARY,
        "enum" | "set" => icon::LIST_BULLETS,
        "geometry" | "point" | "linestring" | "polygon" => icon::SQUARES_FOUR,
        _ => icon::TEXT_T,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_type_glyph_maps_numeric_families_to_hash() {
        for type_name in ["int", "INT(11)", "bigint unsigned", "decimal(10,2)", "double", "year"] {
            assert_eq!(column_type_glyph(type_name), icon::HASH, "{type_name}");
        }
    }

    #[test]
    fn column_type_glyph_treats_tinyint_one_as_boolean_but_other_tinyint_as_number() {
        assert_eq!(column_type_glyph("tinyint(1)"), icon::TOGGLE_LEFT);
        assert_eq!(column_type_glyph("tinyint(4) unsigned"), icon::HASH);
        assert_eq!(column_type_glyph("tinyint"), icon::HASH);
    }

    #[test]
    fn column_type_glyph_distinguishes_temporal_types() {
        assert_eq!(column_type_glyph("date"), icon::CALENDAR_BLANK);
        assert_eq!(column_type_glyph("datetime(6)"), icon::CLOCK);
        assert_eq!(column_type_glyph("timestamp"), icon::CLOCK);
    }

    #[test]
    fn column_type_glyph_covers_json_binary_enum_and_falls_back_to_text() {
        assert_eq!(column_type_glyph("json"), icon::BRACKETS_CURLY);
        assert_eq!(column_type_glyph("longblob"), icon::BINARY);
        assert_eq!(column_type_glyph("enum('a','b')"), icon::LIST_BULLETS);
        assert_eq!(column_type_glyph("varchar(255)"), icon::TEXT_T);
        assert_eq!(column_type_glyph(""), icon::TEXT_T);
    }
}

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime};
use cottondata_grid::CellValue;
use std::fmt::Write;
use tiberius::{ColumnData, ColumnType, FromSql};

pub(crate) fn cell_value(data: &ColumnData<'static>) -> CellValue {
    match column_text(data) {
        Some(text) => CellValue::from_text(text),
        None => CellValue::Null,
    }
}

pub(crate) fn column_text(data: &ColumnData<'static>) -> Option<String> {
    match data {
        ColumnData::U8(value) => value.map(|number| number.to_string()),
        ColumnData::I16(value) => value.map(|number| number.to_string()),
        ColumnData::I32(value) => value.map(|number| number.to_string()),
        ColumnData::I64(value) => value.map(|number| number.to_string()),
        ColumnData::F32(value) => value.map(|number| number.to_string()),
        ColumnData::F64(value) => value.map(|number| number.to_string()),
        ColumnData::Bit(value) => value.map(|flag| if flag { "1" } else { "0" }.to_string()),
        ColumnData::String(value) => value.as_ref().map(|text| text.to_string()),
        ColumnData::Guid(value) => value.map(|guid| guid.to_string().to_uppercase()),
        ColumnData::Binary(value) => value.as_ref().map(|bytes| hex_literal(bytes)),
        ColumnData::Numeric(value) => value.map(|number| number.to_string()),
        ColumnData::Xml(value) => value.as_ref().map(|xml| xml.to_string()),
        ColumnData::DateTime(_) | ColumnData::SmallDateTime(_) | ColumnData::DateTime2(_) => {
            decode::<NaiveDateTime>(data).map(|timestamp| timestamp.format("%Y-%m-%d %H:%M:%S%.f").to_string())
        }
        ColumnData::Date(_) => decode::<NaiveDate>(data).map(|date| date.format("%Y-%m-%d").to_string()),
        ColumnData::Time(_) => decode::<NaiveTime>(data).map(|time| time.format("%H:%M:%S%.f").to_string()),
        ColumnData::DateTimeOffset(_) => decode::<DateTime<FixedOffset>>(data)
            .map(|timestamp| timestamp.format("%Y-%m-%d %H:%M:%S%.f %:z").to_string()),
    }
}

fn decode<'a, T>(data: &'a ColumnData<'static>) -> Option<T>
where
    T: FromSql<'a>,
{
    T::from_sql(data).ok().flatten()
}

fn hex_literal(bytes: &[u8]) -> String {
    let mut literal = String::with_capacity(2 + bytes.len() * 2);
    literal.push_str("0x");
    for byte in bytes {
        let _ = write!(literal, "{byte:02X}");
    }
    literal
}

pub(crate) fn column_type_label(column_type: ColumnType) -> &'static str {
    match column_type {
        ColumnType::Null => "null",
        ColumnType::Bit | ColumnType::Bitn => "bit",
        ColumnType::Int1 => "tinyint",
        ColumnType::Int2 => "smallint",
        ColumnType::Int4 | ColumnType::Intn => "int",
        ColumnType::Int8 => "bigint",
        ColumnType::Float4 => "real",
        ColumnType::Float8 | ColumnType::Floatn => "float",
        ColumnType::Money | ColumnType::Money4 => "money",
        ColumnType::Decimaln | ColumnType::Numericn => "decimal",
        ColumnType::Datetime4 => "smalldatetime",
        ColumnType::Datetime | ColumnType::Datetimen => "datetime",
        ColumnType::Daten => "date",
        ColumnType::Timen => "time",
        ColumnType::Datetime2 => "datetime2",
        ColumnType::DatetimeOffsetn => "datetimeoffset",
        ColumnType::Guid => "uniqueidentifier",
        ColumnType::BigVarBin | ColumnType::BigBinary | ColumnType::Image => "varbinary",
        ColumnType::BigVarChar | ColumnType::BigChar | ColumnType::Text => "varchar",
        ColumnType::NVarchar | ColumnType::NChar | ColumnType::NText => "nvarchar",
        ColumnType::Xml => "xml",
        ColumnType::Udt => "udt",
        ColumnType::SSVariant => "sql_variant",
    }
}

pub(crate) fn is_numeric_type(column_type: ColumnType) -> bool {
    matches!(
        column_type,
        ColumnType::Int1
            | ColumnType::Int2
            | ColumnType::Int4
            | ColumnType::Int8
            | ColumnType::Intn
            | ColumnType::Float4
            | ColumnType::Float8
            | ColumnType::Floatn
            | ColumnType::Money
            | ColumnType::Money4
            | ColumnType::Decimaln
            | ColumnType::Numericn
    )
}

pub(crate) fn format_declared_type(type_name: &str, max_length: i16, precision: u8, scale: u8) -> String {
    let lower = type_name.to_ascii_lowercase();
    match lower.as_str() {
        "varchar" | "char" | "varbinary" | "binary" => format!("{lower}({})", length_label(max_length, 1)),
        "nvarchar" | "nchar" => format!("{lower}({})", length_label(max_length, 2)),
        "decimal" | "numeric" => format!("{lower}({precision},{scale})"),
        "datetime2" | "time" | "datetimeoffset" => format!("{lower}({scale})"),
        _ => lower,
    }
}

fn length_label(max_length: i16, bytes_per_character: i16) -> String {
    if max_length < 0 {
        "max".into()
    } else {
        (max_length / bytes_per_character).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    #[test]
    fn converts_scalar_column_data_to_text_and_null() {
        assert_eq!(column_text(&ColumnData::I32(Some(42))), Some("42".into()));
        assert_eq!(column_text(&ColumnData::I64(None)), None);
        assert_eq!(column_text(&ColumnData::Bit(Some(true))), Some("1".into()));
        assert_eq!(
            column_text(&ColumnData::String(Some(Cow::Borrowed("héllo")))),
            Some("héllo".into())
        );
        assert_eq!(
            column_text(&ColumnData::Binary(Some(Cow::Borrowed(&[0x0A, 0xFF])))),
            Some("0x0AFF".into())
        );
    }

    #[test]
    fn decimals_keep_their_scale_as_text() {
        let numeric = tiberius::numeric::Numeric::new_with_scale(1250, 2);
        assert_eq!(column_text(&ColumnData::Numeric(Some(numeric))), Some("12.50".into()));
    }

    #[test]
    fn formats_declared_types_with_lengths_precision_and_max() {
        assert_eq!(format_declared_type("nvarchar", 200, 0, 0), "nvarchar(100)");
        assert_eq!(format_declared_type("varchar", -1, 0, 0), "varchar(max)");
        assert_eq!(format_declared_type("decimal", 9, 18, 2), "decimal(18,2)");
        assert_eq!(format_declared_type("datetime2", 8, 27, 7), "datetime2(7)");
        assert_eq!(format_declared_type("INT", 4, 10, 0), "int");
    }

    #[test]
    fn numeric_classification_drives_right_alignment_and_json_numbers() {
        assert!(is_numeric_type(ColumnType::Int8));
        assert!(is_numeric_type(ColumnType::Decimaln));
        assert!(!is_numeric_type(ColumnType::NVarchar));
        assert_eq!(column_type_label(ColumnType::NVarchar), "nvarchar");
        assert_eq!(column_type_label(ColumnType::Guid), "uniqueidentifier");
    }
}

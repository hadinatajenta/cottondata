use tiberius::{Client, Row};
use tokio::net::TcpStream;
use tokio_util::compat::Compat;

pub(crate) type MssqlClient = Client<Compat<TcpStream>>;

pub(crate) fn optional_text(row: &Row, index: usize) -> Option<String> {
    row.try_get::<&str, _>(index).ok().flatten().map(str::to_string)
}

pub(crate) fn text(row: &Row, index: usize) -> String {
    optional_text(row, index).unwrap_or_default()
}

pub(crate) fn int(row: &Row, index: usize) -> i64 {
    if let Ok(Some(value)) = row.try_get::<i64, _>(index) {
        return value;
    }
    if let Ok(Some(value)) = row.try_get::<i32, _>(index) {
        return i64::from(value);
    }
    if let Ok(Some(value)) = row.try_get::<i16, _>(index) {
        return i64::from(value);
    }
    if let Ok(Some(value)) = row.try_get::<u8, _>(index) {
        return i64::from(value);
    }
    0
}

pub(crate) fn flag(row: &Row, index: usize) -> bool {
    row.try_get::<bool, _>(index).ok().flatten().unwrap_or(false)
}

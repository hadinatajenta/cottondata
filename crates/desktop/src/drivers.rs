use cottondata_core::{DatabaseEngine, ServerProfile};
use cottondata_driver::{DatabaseSession, DbError};
use cottondata_mysql::MySqlSession;

pub fn open_session(profile: &ServerProfile, password: Option<&str>) -> Result<Box<dyn DatabaseSession>, DbError> {
    match profile.engine {
        DatabaseEngine::MySql => Ok(Box::new(MySqlSession::connect(profile, password)?)),
        DatabaseEngine::SqlServer => Err(unsupported_engine(profile.engine)),
    }
}

pub fn kill_running_query(profile: &ServerProfile, password: Option<&str>, connection_id: u32) -> Result<(), DbError> {
    match profile.engine {
        DatabaseEngine::MySql => MySqlSession::kill_query_via_aux(profile, password, connection_id),
        DatabaseEngine::SqlServer => Err(unsupported_engine(profile.engine)),
    }
}

fn unsupported_engine(engine: DatabaseEngine) -> DbError {
    DbError::message(format!("{} connections are not available in this build", engine.label()))
}

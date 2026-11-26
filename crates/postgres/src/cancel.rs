use crate::query::OverflowCanceller;
use crate::session::PostgresSession;
use cottondata_core::{ServerProfile, SqlDialect};
use cottondata_driver::DbError;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use tokio_postgres::CancelToken;
use tokio_postgres_rustls::MakeRustlsConnect;

#[derive(Clone)]
pub(crate) struct CancelHandle {
    pub(crate) token: CancelToken,
    pub(crate) tls: MakeRustlsConnect,
    pub(crate) cockroach_session_id: Option<String>,
}

type RegistryKey = (String, u32);

static REGISTRY: OnceLock<Mutex<HashMap<RegistryKey, CancelHandle>>> = OnceLock::new();
static NEXT_SYNTHETIC_ID: AtomicU32 = AtomicU32::new(1);

fn registry() -> &'static Mutex<HashMap<RegistryKey, CancelHandle>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(crate) fn next_synthetic_connection_id() -> u32 {
    NEXT_SYNTHETIC_ID.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn register(profile_id: &str, connection_id: u32, handle: CancelHandle) {
    if let Ok(mut entries) = registry().lock() {
        entries.insert((profile_id.to_string(), connection_id), handle);
    }
}

pub(crate) fn unregister(profile_id: &str, connection_id: u32) {
    if let Ok(mut entries) = registry().lock() {
        entries.remove(&(profile_id.to_string(), connection_id));
    }
}

pub(crate) fn lookup(profile_id: &str, connection_id: u32) -> Option<CancelHandle> {
    registry()
        .lock()
        .ok()
        .and_then(|entries| entries.get(&(profile_id.to_string(), connection_id)).cloned())
}

pub(crate) fn send_cancel_request(handle: &CancelHandle) -> Result<(), DbError> {
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| DbError::message(format!("Could not start the cancel runtime: {err}")))?;
    executor
        .block_on(handle.token.cancel_query(handle.tls.clone()))
        .map_err(crate::errors::postgres_error)
}

impl PostgresSession {
    pub fn kill_query_via_aux(profile: &ServerProfile, password: Option<&str>, connection_id: u32) -> Result<(), DbError> {
        let handle = lookup(&profile.id, connection_id);
        let is_cockroach_session = handle.as_ref().is_some_and(|found| found.cockroach_session_id.is_some());
        let cancel_error = match handle.as_ref().filter(|_| !is_cockroach_session).map(send_cancel_request) {
            Some(Ok(())) => return Ok(()),
            Some(Err(error)) => Some(error),
            None => None,
        };
        match handle.and_then(|found| found.cockroach_session_id) {
            Some(session_id) => PostgresSession::cancel_cockroach_session_queries(profile, password, &session_id),
            None => {
                let mut aux = PostgresSession::connect(profile, password)?;
                if aux.is_cockroach() {
                    return Err(cancel_error
                        .unwrap_or_else(|| DbError::message("The running CockroachDB session was not found")));
                }
                aux.run_batch(&format!("SELECT pg_cancel_backend({connection_id})"))
            }
        }
    }

    pub(crate) fn cancel_cockroach_session_queries(
        profile: &ServerProfile,
        password: Option<&str>,
        session_id: &str,
    ) -> Result<(), DbError> {
        let mut aux = PostgresSession::connect(profile, password)?;
        aux.run_batch(&format!(
            "CANCEL QUERIES IF EXISTS (SELECT query_id FROM [SHOW CLUSTER QUERIES] WHERE session_id = {})",
            SqlDialect::CockroachDb.string_literal(session_id)
        ))
    }
}

pub(crate) fn cockroach_overflow_canceller(
    profile: &ServerProfile,
    password: Option<&str>,
    session_id: String,
) -> Box<OverflowCanceller> {
    let profile = profile.clone();
    let password = password.map(str::to_string);
    Box::new(move || {
        let profile = profile.clone();
        let password = password.clone();
        let session_id = session_id.clone();
        std::thread::spawn(move || {
            let _ = PostgresSession::cancel_cockroach_session_queries(&profile, password.as_deref(), &session_id);
        })
    })
}

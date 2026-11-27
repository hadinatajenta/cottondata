pub mod budget;
pub mod diagnostics;
pub mod dialect;
pub mod registry;

pub use budget::{BudgetStats, MemoryBudget, DEFAULT_GRID_BUDGET_BYTES, MAX_INLINE_CELL_BYTES};
pub use diagnostics::{DbDiagnostic, DiagnosticSeverity, SqlErrorLocation, SqlErrorObjectKind};
pub use dialect::{DatabaseEngine, SqlDialect};
pub use registry::{
    ConnectionEndpoint, ConnectionStatus, EnvironmentTag, ServerProfile, ServerRegistry,
    ServerRuntimeInfo, ServerSlot, ServerSlotId, SshTunnelConfig, TlsMode, DEFAULT_MAX_SERVERS,
    DEFAULT_PAGE_SIZE, MAX_ENVIRONMENT_LABEL_CHARS, MAX_SOCKETS_PER_SERVER, SERVER_SLOT_CAPACITY,
    UNLIMITED_ROWS,
};

# Changelog

All notable changes to CottonData are documented in this file.

## [Unreleased]

### Added
- **PostgreSQL driver (`crates/postgres`)** for PostgreSQL 9.6+ and compatible services (Supabase, Neon, TimescaleDB): cursor-based paging so large `SELECT`s only fetch one page, inline editing of single-table `SELECT` results using the row description's origin table and key metadata, booleans as `true`/`false`, `bytea` as `\x…`, arrays and JSONB as text, `pg_get_*def`-accurate DDL with indexes, constraints and comments, views and materialized views, routines, inbound and outbound foreign keys, `pg_stat_activity` process list with cancel, health metrics, `EXPLAIN` and `EXPLAIN (ANALYZE, BUFFERS)` that rolls back data changes, server-side Read-Only via `default_transaction_read_only`, emulated manual-commit mode, savepoint-safe grid saves inside open transactions, Stop through the native cancel protocol, SQLSTATE-aware errors with hints and line/column positions, all five TLS modes (with correct SNI through SSH tunnels), and Unix sockets.
- **CockroachDB support** on the same driver with automatic detection: `SHOW CREATE TABLE` DDL, `SHOW INDEXES`, `SHOW CLUSTER QUERIES` process list with `CANCEL QUERY`, cluster health, and page-capped reads that cancel the remainder of a large result instead of streaming it.
- **SQLite driver (`crates/sqlite`)**: open or create database files, attached databases as schemas, foreign keys enforced, page limits without full scans, editable results via column metadata, triggers, indexes and foreign keys (both directions), `EXPLAIN QUERY PLAN` tree plus a timed run that is rolled back, health from PRAGMAs, and Stop via interrupt.
- Connection dialog: engine cards for all five engines, `postgres://` / `postgresql://` / `cockroachdb://` / `sqlite:` URLs (libpq `sslmode` and `sslrootcert` included, CockroachDB detected from port 26257 or `*.cockroachlabs.cloud`), and a database-file picker with Browse and New for SQLite.

### Changed
- SQL statement splitter understands PostgreSQL dollar-quoted bodies and JSONB `#>`/`#>>` operators; `?` is no longer treated as a query parameter on PostgreSQL-family connections because it is a JSONB operator there.
- Table Structure Editor and Schema Compare generate PostgreSQL, CockroachDB and SQLite DDL, and explain clearly when SQLite needs a table rebuild.
- Full-table-scan warnings in the Explain panel recognise PostgreSQL, CockroachDB and SQLite plans.
- Host/port/SSH-tunnel resolution is shared by all network drivers (`cottondata-driver::resolve_tcp_target`), and killing a process is a `DatabaseSession` method so each engine can cancel in its own way.

## [0.9.0] — 2026-11-26

### Changed
- **Typography**: the UI now uses **Inter** and the SQL editor and result grid use **JetBrains Mono**, both embedded under the SIL Open Font License (`assets/fonts/`).
- **Unified macOS titlebar**: the window content extends under a transparent titlebar, so the native title strip no longer clashes with the dark theme. The top strip still drags the window, and double-click toggles zoom.
- **Calmer toolbars**: ghost button icons use the muted text color; tinted buttons (Stop, Commit, Rollback) only show their color while enabled.
- **Fewer borders**: cards are separated by fill and a soft shadow instead of a 1px border, and inset panels use a softer divider stroke.

## [0.8.1] — 2026-11-26

### Changed
- **SQL autocomplete popup (`completion_popup.rs`)**: completions now appear in a floating popup anchored at the caret instead of a button row under the editor, with kind icons (table, column, function, keyword, system variable), matched-prefix highlight, and right-aligned details. Navigate with `↑/↓`, accept with `Enter`/`Tab`, dismiss with `Esc`, reopen with `Ctrl+Space`.

### Fixed
- Accepting a completion now moves the caret to the end of the inserted word and keeps editor focus.

## [0.8.0] — 2026-11-26

### Added
- **Table Structure Editor (`crates/sql/src/ddl_builder.rs`, `structure_editor.rs`)**: add, rename, modify, or drop columns, indexes, and foreign keys through a tabbed form with live syntax-highlighted `ALTER TABLE` SQL preview (MySQL & SQL Server), *Open in SQL Console*, and PROD safety confirmation.
- **Streaming CSV / TSV / JSON / JSONL Import (`crates/grid/src/import.rs`, `import_modal.rs`)**: constant-memory file import wizard with RFC 4180 CSV/TSV & streaming JSON parsers, auto-column mapping, `Empty → NULL` controls, full-file **Dry-Run Validation** (NOT NULL & numeric checks), and batched multi-row `INSERT` execution inside an atomic transaction with live row progress and Cancel/rollback.
- **Cross-Server Schema Compare (`crates/introspect/src/diff.rs`, `schema_compare.rs`)**: compare any two servers/schemas (e.g., DEV vs STAGING) to view new, modified, and extra tables, column/index/FK diffs, and generate a migration SQL script (with optional destructive `DROP` toggle) ready to open in SQL Console.
- **Read-Only ER Diagram (`er_diagram.rs`)**: interactive pan/zoom schema ER diagram canvas displaying table cards with PK/FK badges and Bézier foreign key relationship lines, table search filter, relation highlighting on click, and double-click to open table data.

### Fixed
- **Windows & Linux cross-platform polish**: OS-aware shortcut hints (`Ctrl` / `Alt` on Windows/Linux instead of hardcoded macOS `⌘` / `⌥` in grid context menus), `CREATE_NO_WINDOW` flag for Windows SSH tunnel processes (`crates/driver/src/tunnel.rs`), and updated Windows binary metadata in `crates/desktop/build.rs`.

## [0.7.0] — 2026-11-26

### Added
- **Foreign Key Navigation (Outbound & Inbound)**:
  - Introspects both outbound and inbound foreign keys (`InboundForeignKeyInfo`) for MySQL (`information_schema.key_column_usage`) and SQL Server (`sys.foreign_keys`), quietly preloading table metadata for open table tabs and single-table `SELECT` console results.
  - Outbound FK cells display link styling and hover tooltips; `Cmd/Ctrl+Click` or right-click → *Go to `table.column`* opens the referenced table pre-filtered to that value.
  - Right-click any referenced key column → *Find referencing rows* lists all child tables pointing to that column and opens a filtered tab in one click.
  - DDL & Indexes inspector now also lists **Referenced by** inbound foreign keys.
- **Parameterized Queries (`:name` and `?`)**:
  - Lexer-aware parameter extractor and dialect-safe binder in `crates/sql/src/params.rs` (ignores string literals, quoted identifiers, comments, `:=`, and `::`; preserves leading-zero codes like `"01"` as strings in `Auto` mode).
  - Running a parameterized query opens a `modal_shell` **Query Parameters** dialog with value inputs, type selectors (`Auto`, `Text`, `Number`, `NULL`, `Raw SQL`), live syntax-highlighted bound SQL preview, `Cmd/Ctrl+Enter` execution, and automatic per-query value memory.

## [0.6.0] — 2026-11-26

### Added
- **Find & Replace in SQL Editor (`Cmd/Ctrl+F` / `Cmd/Ctrl+H`)**: inline find/replace bar with live syntax-preserving match highlights, active-match contrast, Case-Sensitive (`Aa`) and Whole-Word (`W`) toggles, match counter, Prev/Next navigation, Replace, and Replace All (`crates/sql/src/edit.rs`).
- **SQL Editor Line Editing Shortcuts**: Toggle Line Comment (`Cmd/Ctrl+/`), Duplicate Line / Selection (`Cmd/Ctrl+Shift+D`), and Move Line Up/Down (`Alt+↑` / `Alt+↓`) with accurate UTF-8 cursor and selection tracking.
- **Searchable History & Saved Queries / Snippets**: real-time search + OK/Failed status filter + *Open in New Tab* / *Save as Snippet* in History, plus a new **Saved Queries** deck (`snippets.json`) with Global or per-server scope, folders, pin-to-top, and `Cmd/Ctrl+Shift+S` save modal.
- **Constant-Memory Streaming Full-Result Export**: `DatabaseSession::stream_query` (MySQL `query_iter` + SQL Server `QueryStream`) paired with `StreamExporter` streams entire tables or full query results directly to CSV, TSV, JSON Lines, JSON Array, or SQL `INSERT` files on a background thread with live row progress and Cancel support while staying well within the 32 MiB RAM guardrail.

## [0.5.1] — 2026-11-26

### Fixed
- Pixel-aligned `option_card` and `toggle_card` icons, checkboxes, and text vertically to card center, and switched side-by-side card rows (Database Engine, TLS Mode, Theme selector) to top-aligned horizontal layout.

## [0.5.0] — 2026-11-26

### Added
- **Premium Modal System (`ui_kit/modal.rs`, `ui_kit/form.rs`)**: centered dialog cards with dimmed backdrop scrim, shadow, icon badge header + subtitle, `Esc` dismiss, and styled form primitives across all dialogs (New/Edit Connection, Settings, Workspace Sessions, Preview Staged Transaction, Unsaved Cell Changes, and Compare Rows).
- **Redesigned Connection Form**: tabbed **General / Security & TLS / SSH Tunnel** layout, selectable Engine cards (`MySQL` / `SQL Server`), color-coded Environment pills, show/hide password toggle, **Paste Connection URL** parser (`mysql://`, `mariadb://`, `mssql://`, `sqlserver://`), live **Test Connection** button with latency and server version readout, `Cmd/Ctrl+Enter` save, and right-click **Duplicate server…** in Database Explorer.
- **Reopen Closed Tab (`Cmd/Ctrl+Shift+T`)**: restores up to 10 recently closed console/table tabs.
- **Syntax-highlighted SQL Preview**: staged transaction preview now highlights SQL, adds a Copy SQL button, and displays a Production warning banner when targeting PROD servers.

### Fixed
- **Reliable Workspace Auto-Save**: flushes `sessions.json` immediately on `ViewportEvent::CloseRequested` (`Cmd+Q` / window close), lowers typing debounce from 1200 ms to 400 ms, persists on query execution and editor focus loss, saves `editor_height` and per-tab `show_sql_editor` state, and surfaces disk save errors instead of silently ignoring them.

## [0.4.4] — 2026-11-26

### Fixed
- Auto-dismissing toast notifications (`Success` / `Info` 4 s, `Warning` 6 s, `Error` 10 s) with hover-to-pause and reactive repaint scheduling.
- Show actual server names (`Connected to #4 <name>`) instead of generic slot numbers, use warning tone for lost connections, and automatically clear stale error banners when a new query runs or succeeds.
- Wrapped long banner messages cleanly so they never overlap the dismiss `×` button.

## [0.4.3] — 2026-11-26

### Fixed
- Responsive top toolbar layout (`TopBarLayout`) with calibrated label breakpoints, truncated `ComboBox` labels, and clipped right-hand controls so badges and server pickers never overlap transaction buttons (`Rollback`) on narrower windows.

## [0.4.2] — 2026-11-26

### Fixed
- Prevented `Database Explorer` header text from overlapping the `System` toggle on narrow sidebars.
- Prevented the left sidebar from auto-expanding horizontally while typing in the search filter.
- Fixed fast typing lag and focus loss in the Explorer filter via stable explicit widget IDs, zero-allocation case-insensitive matching, per-schema match capping while filtering, and viewport culling for offscreen table rows.

## [0.4.1] — 2026-11-26

### Fixed
- Eliminated black framebuffer gap on the sidebar right edge during Explorer search overflow and matched `eframe::App::clear_color` to the active theme backdrop.

## [0.4.0] — 2026-11-26

### Added
- **Multi-Result Sub-Tabs**: `Run all` (`Cmd/Ctrl+Shift+Enter`) and `Run selected` execute multiple statements and open independent *Result 1*, *Result 2*, … sub-tabs.
- **SQL Editor Upgrades**: line numbers gutter with active-line highlight, visible 12 px draggable editor splitter, and expand/compact editor toggle.
- **Grid Column Drag-Resize**: drag column dividers to resize and double-click to auto-fit.
- **Vertical Record View Drawer**: right-click any row → *View row as list* with column search, *Hide NULLs*, *Pretty JSON*, Prev/Next row navigation, inline editing, and Copy JSON.
- **Compare 2 Rows Diff**, **Quick Filter** (`=` / `!=`), and **Copy column as `IN (...)`**.

## [0.3.0] — 2026-11-26

### Added
- **DataGrip-style Inline Cell Editing**: inline editing directly inside table tabs and single-table `SELECT` console results (`Double-click`, `Enter`, `F2`, or type-to-edit), `org_name` tracking, `Unique NOT NULL` key fallback, `Set NULL`/`DEFAULT`, lazy full-cell fetch for >4 KiB truncated values, `SAVEPOINT`-safe atomic commits, direct `Cmd/Ctrl+S` on non-PROD + mandatory SQL Preview on PROD, and Unsaved Changes Guard modal.
- **Unified Database Explorer**: merged Connections and Schema Navigator into one full-height tree with auto-connect on expand, cross-server filtering, routines display, and full right-click menus.
- **Workspace Polish**: auto-collapsed SQL editor on table tabs (`Show SQL` toggle), environment color accents on document tabs, `EXPLAIN` guard for non-explainable statements (`SHOW CREATE TABLE`, etc.), and fixed Read-Only guard for parenthesized `SELECT`/`UNION` and `TABLE` queries.
- Replaced GitHub Actions release workflows with local release helper `scripts/publish-release.sh`.

## [0.2.0] — 2026-11-25

### Added
- **SQL Server Support** via `tiberius` + `rustls` with shared `DatabaseSession` driver abstraction (`crates/driver`, `crates/mssql`).
- **Configurable Server Limit** (1–64 saved servers) in Settings.
- **Persistent Workspace Sessions** with keep-alive ping and automatic reconnect.
- **Reference-style UI**, aligned Data Grid, row selection & copy/export, and progress indicators.

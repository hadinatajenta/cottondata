# Changelog

All notable changes to CottonData are documented in this file.

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

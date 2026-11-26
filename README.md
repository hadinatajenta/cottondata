<p align="center">
  <img src="assets/AppIcon-256.png" width="128" height="128" alt="CottonData Liquid Glass App Icon" />
</p>

<h1 align="center">CottonData</h1>

<p align="center">
  <strong>Featherweight Native Rust Client for MySQL and SQL Server. Up to 64 Servers, Zero JVM Bloat.</strong>
</p>

---

## Why CottonData?

Traditional JVM database IDEs (such as DataGrip) ship multi-gigabyte bundles with embedded Chromium, dozens of unused DBMS dialects, ML completion runtimes, and out-of-process RMI JDBC JVMs — routinely consuming **1.5–3.5 GB of RAM** when working across multiple servers.

**CottonData** is a clean-room native Rust rewrite built specifically for one job: **fast, low-RAM database engineering across MySQL and SQL Server, with a server limit you choose (1 to 64, default 10)**.

| Dimension | CottonData (Rust) | Typical JVM DB IDE |
|---|---|---|
| Supported Engines | **MySQL** (5.7 / 8.0 / 8.4 / 9.x) and **SQL Server** (2012+, SQL login) | 30+ dialects & drivers |
| Server Slot Cap | **User setting: 1 to 64 servers, default 10** (`Cmd/Ctrl+1..0` for the first ten) | Unbounded heap & pools |
| Runtime | Single native binary + `mimalloc` + `rustls` | JVM + JCEF Chromium + Remote JDBC JVM |
| Disk Footprint | **~10–15 MB** binary | ~2.4 GB bundle |
| RAM Guardrail | **32 MiB global LRU grid budget** + 4 KiB inline cell cap | 1.5–3.5+ GB RSS |
| Platforms | **macOS (arm64 + x64), Linux (x64), Windows (x64)** | Heavyweight per-platform JBR |

---

## Key Features

- **Strict 10-Server Fixed Slot Architecture (`MAX_SERVERS = 10`):**
  - Switch instantly with `Cmd/Ctrl + 1..0`
  - Environment badges (`LOCAL`, `DEV`, `STAGING`, `PROD`) & client-side **Read-Only Guard**
  - Lazy connections (0 TCP sockets on launch; max 2 sockets per active server: primary session + ephemeral `KILL QUERY` aux socket)
  - TCP/IP, Unix Domain Sockets (`/tmp/mysql.sock`), pure-Rust TLS (`rustls`), SSH Tunneling, and macOS Keychain password storage
- **Multilevel & Lazy Schema Introspection:**
  - Level 0 (`information_schema.schemata`) separates system schemas (`mysql`, `information_schema`, `performance_schema`, `sys`)
  - Level 1 loads tables, views, columns, and routines on demand into a string-interned (`lasso`) catalog
  - Level 2 fetches `SHOW CREATE TABLE`, indexes, and foreign keys lazily on click
- **MySQL-Only SQL Console & Safety Engine:**
  - Syntax highlighting, `DELIMITER`-aware statement splitting, execute-at-cursor (`Cmd/Ctrl+Enter`), SQL formatter, and schema/alias-aware completion (`SELECT o.<tab> FROM orders o`)
  - Safety alerts for unqualified `UPDATE`/`DELETE` without `WHERE`, `DROP`, and `TRUNCATE`
  - Query cancellation via ephemeral `KILL QUERY <connection_id>`
- **Compact Columnar Data Grid & Safe Staged Mutator:**
  - Single contiguous `text_arena` + 1-bit null bitmap + 4 KiB BLOB/TEXT inline preview cap
  - Server-side `WHERE` filter, column `ORDER BY`, 500-row pagination, and lazy `COUNT(*)`
  - Local staged `INSERT` / `Clone Row` / `UPDATE` / `DELETE` overlay with **Preview SQL** and atomic `START TRANSACTION -> DELETE -> UPDATE -> INSERT -> COMMIT`
  - Streaming export to **CSV, JSON Lines, JSON Array, and SQL `INSERT`**
- **Diagnostics & Operations:**
  - `EXPLAIN FORMAT=JSON` & `EXPLAIN ANALYZE` (MySQL ≥ 8.0.18) with Full Table Scan warnings
  - Live `SHOW FULL PROCESSLIST` with 1-click `KILL QUERY` and InnoDB Buffer Pool Hit Ratio metrics
- **Liquid Glass UI:**
  - Dark Glass & Light Glass themes paired with a custom macOS Tahoe Liquid Glass cotton icon across `.icns`, `.ico`, and Linux `.png`

---

## Workspace Crates

```text
crates/
├── core/        # MAX_SERVERS=10 ServerRegistry, MemoryBudget LRU, MySQL error diagnostics
├── mysql/       # Pure-Rust MySQL client (rustls-tls-ring), streaming rows, SSH tunnel, KILL QUERY
├── introspect/  # Multilevel L0/L1/L2 MySQL introspector + lasso string-interned catalog
├── sql/         # Zero-copy MySQL lexer, DELIMITER splitter, safety linter, alias autocomplete, formatter
├── grid/        # ColumnarPage arena buffer, StagedMutator overlay, streaming CSV/JSON/SQL export
├── storage/     # 10-slot profile persistence, query history, macOS Keychain integration
├── cli/         # Headless `cottondata-cli` binary
└── desktop/     # Native `CottonData` GUI (eframe/egui, Liquid Glass theme, Windows winresource)
```

---

## Build & Release Packaging

### Run Tests & Local Build
```bash
cargo test --workspace
cargo build --release -p cottondata-desktop -p cottondata-cli
```

### Package Releases
- **macOS (`.app`, `.zip`, `.dmg`):**
  ```bash
  ./scripts/bundle-macos.sh
  ```
- **Linux (`.tar.gz`, `.deb`, `.desktop`):**
  ```bash
  ./scripts/bundle-linux.sh
  ```
- **Windows (`.zip` + Inno Setup `.exe`):**
  ```powershell
  .\scripts\bundle-windows.ps1
  iscc scripts\installer-windows.iss
  ```

### Automated Cross-Platform Releases
Push a semantic version tag (`git tag v0.1.0 && git push origin v0.1.0`) or run the **Release Build (macOS + Linux + Windows)** GitHub Action manually to build and publish signed/checksummed release bundles for macOS (`arm64` & `x64`), Linux (`x64`), and Windows (`x64`).

---

## CLI Quick Start

```bash
cottondata-cli doctor
cottondata-cli servers add local-db 127.0.0.1:3306 root myapp
cottondata-cli servers list
COTTONDATA_PASSWORD=secret cottondata-cli test local-db
COTTONDATA_PASSWORD=secret cottondata-cli query local-db "SELECT NOW()" --format csv
```

## License

Dual-licensed under **MIT** or **Apache-2.0**.

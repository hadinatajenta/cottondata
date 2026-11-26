<p align="center">
  <img src="assets/AppIcon-256.png" width="128" height="128" alt="CottonData app icon" />
</p>

<h1 align="center">CottonData</h1>

<p align="center">
  <strong>Featherweight native Rust desktop client for MySQL and SQL Server.</strong><br/>
  Single ~13 MB app, no JVM, no embedded browser.
</p>

---

## Install

Pick the row that matches the machine, then run that section's commands top to bottom.
Every block is non-interactive, needs no `sudo` unless noted, and ends with a check you can verify.

| OS | CPU | Method | Section |
|---|---|---|---|
| macOS 12+ | Apple Silicon (`arm64`) | Prebuilt release | [macOS Apple Silicon](#macos-apple-silicon-prebuilt) |
| macOS 12+ | Intel (`x86_64`) | Build from source | [macOS Intel](#macos-intel-build-from-source) |
| Linux | `x86_64` or `aarch64` | Build from source | [Linux](#linux-build-from-source) |
| Windows 10/11 | `x86_64` | Build from source | [Windows](#windows-build-from-source) |

Detect the platform:

```bash
uname -s   # Darwin or Linux
uname -m   # arm64, x86_64, or aarch64
```

On Windows PowerShell: `$env:PROCESSOR_ARCHITECTURE` (expect `AMD64`).

### For AI agents

```yaml
repository: https://github.com/hadinatajenta/cottondata
latest_release_page: https://github.com/hadinatajenta/cottondata/releases/latest
prebuilt_assets:
  macos_arm64_zip: https://github.com/hadinatajenta/cottondata/releases/latest/download/CottonData-macos-arm64.zip
  macos_arm64_dmg: https://github.com/hadinatajenta/cottondata/releases/latest/download/CottonData-macos-arm64.dmg
prebuilt_platforms: [macos-arm64]
build_from_source_platforms: [macos-x86_64, linux-x86_64, linux-aarch64, windows-x86_64]
minimum_rust: "1.88"
gui_binary: CottonData            # CottonData.exe on Windows
cli_binary: cottondata-cli        # cottondata-cli.exe on Windows
verify_command: cottondata-cli doctor   # prints "CottonData CLI v<version>"
database_server_required_for_install: false
display_required_for_gui: true
```

Rules that keep installs fast and correct:

1. Use the prebuilt release on macOS Apple Silicon. Building there takes several minutes for no benefit.
2. On every other platform, install the listed system packages **before** running `cargo`, otherwise the build stops halfway through.
3. Run commands from the repository root. The bundle scripts locate files relative to themselves.
4. The GUI needs a desktop session. On a headless machine, install and verify with `cottondata-cli` only.
5. Do not create database servers or users to "test" the install. `cottondata-cli doctor` needs no server.

### macOS Apple Silicon (prebuilt)

```bash
set -euo pipefail
WORK_DIR="$(mktemp -d)"
curl -fsSL -o "$WORK_DIR/CottonData.zip" \
  https://github.com/hadinatajenta/cottondata/releases/latest/download/CottonData-macos-arm64.zip
ditto -x -k "$WORK_DIR/CottonData.zip" "$WORK_DIR"
rm -rf /Applications/CottonData.app
mv "$WORK_DIR/CottonData.app" /Applications/
xattr -dr com.apple.quarantine /Applications/CottonData.app
rm -rf "$WORK_DIR"
```

Verify:

```bash
/Applications/CottonData.app/Contents/MacOS/cottondata-cli doctor | head -1
# CottonData CLI v0.8.1
open /Applications/CottonData.app
```

The app is ad-hoc signed. `xattr` above removes the download quarantine so Gatekeeper does not block the first launch.
Without it, open the app once with right-click > Open.

### macOS Intel (build from source)

```bash
set -euo pipefail
xcode-select --install 2>/dev/null || true
command -v rustup >/dev/null || curl -fsSL https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup toolchain install stable --profile minimal
git clone https://github.com/hadinatajenta/cottondata.git
cd cottondata
./scripts/bundle-macos.sh
rm -rf /Applications/CottonData.app
cp -R dist/CottonData.app /Applications/
```

Verify:

```bash
/Applications/CottonData.app/Contents/MacOS/cottondata-cli doctor | head -1
```

`bundle-macos.sh` builds for the host CPU and also writes `dist/CottonData-macos-x64.dmg` and `.zip`.

### Linux (build from source)

Debian / Ubuntu:

```bash
set -euo pipefail
sudo apt-get update
sudo apt-get install -y build-essential pkg-config cmake clang git curl \
  libgtk-3-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev
command -v rustup >/dev/null || curl -fsSL https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup toolchain install stable --profile minimal
git clone https://github.com/hadinatajenta/cottondata.git
cd cottondata
if [ "$(uname -m)" = "aarch64" ]; then export ARCH_LABEL=arm64 DEB_ARCH=arm64; fi
./scripts/bundle-linux.sh
"dist/cottondata-linux-${ARCH_LABEL:-x64}/install-user.sh"
```

Fedora / RHEL equivalents for the first two lines:

```bash
sudo dnf install -y gcc gcc-c++ make pkgconf-pkg-config cmake clang git curl \
  gtk3-devel libxcb-devel libxkbcommon-devel
```

Verify:

```bash
"$HOME/.local/bin/cottondata-cli" doctor | head -1
```

The user install puts `CottonData` and `cottondata-cli` in `~/.local/bin` and adds a desktop entry.
On Debian-based systems the script also writes `dist/cottondata_<version>_<arch>.deb`, which can be installed system-wide with `sudo apt-get install ./dist/cottondata_*.deb`.

### Windows (build from source)

PowerShell, as a normal user (the `winget` installs may prompt for elevation):

```powershell
$ErrorActionPreference = "Stop"
winget install --id Rustlang.Rustup -e --accept-source-agreements --accept-package-agreements
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install --id Kitware.CMake -e
winget install --id NASM.NASM -e
winget install --id Git.Git -e
# Open a new PowerShell window so PATH picks up the tools, then:
rustup toolchain install stable --profile minimal
git clone https://github.com/hadinatajenta/cottondata.git
cd cottondata
.\scripts\bundle-windows.ps1
```

Verify:

```powershell
.\dist\CottonData-windows-x64\cottondata-cli.exe doctor | Select-Object -First 1
.\dist\CottonData-windows-x64\CottonData.exe
```

The portable build lives in `dist\CottonData-windows-x64\` and is zipped to `dist\CottonData-windows-x64.zip`.
With [Inno Setup](https://jrsoftware.org/isinfo.php) installed, `iscc scripts\installer-windows.iss` produces an installer.

### Troubleshooting builds

| Symptom | Fix |
|---|---|
| `rustc 1.xx is not supported` / `requires rustc 1.88` | `rustup update stable` |
| Build fails inside `aws-lc-sys` | Install `cmake` and a C compiler (`clang`/MSVC). On Windows also install NASM, then open a new shell |
| `failed to run custom build command for gtk-sys` / missing `gtk-3` | Install the GTK and XCB dev packages listed for your distro |
| `linker 'link.exe' not found` (Windows) | Install Visual Studio Build Tools with the C++ workload |
| macOS says the app is damaged or from an unidentified developer | `xattr -dr com.apple.quarantine /Applications/CottonData.app` |

---

## Features

- **MySQL and SQL Server** in one native app — MySQL 5.7 / 8.x / 9.x / MariaDB and SQL Server 2012+ with SQL login.
- **Unified Database Explorer**: single tree combining saved servers, schemas/databases, tables, views, routines, and columns. Auto-connects on expand, supports right-click actions (including *Duplicate server*), and filters across servers with viewport culling for schemas containing hundreds of tables.
- **Connections & Setup Modal**: tabbed **General / Security & TLS / SSH Tunnel** dialog with live **Test Connection** (latency + server version), **Paste Connection URL** (`mysql://`, `mariadb://`, `mssql://`, `sqlserver://`), show/hide password, Unix sockets (MySQL), all TLS modes (`Disabled` through `Verify Identity`), color-coded environment badges (`LOCAL` / `DEV` / `STAGING` / `PROD`), and per-server **Read-Only Guard**.
- **Configurable server list**: 1 to 64 saved servers (default 10) from Settings. Saved servers cost almost no memory; only connected servers use RAM. `Cmd/Ctrl+1..0` switches the first ten.
- **SQL Console & Multi-Result Tabs**: line-numbered syntax-highlighted editor, **Find & Replace** (`Cmd/Ctrl+F` / `Cmd/Ctrl+H`) with live match highlights, line-editing shortcuts (`Cmd/Ctrl+/` comment, `Cmd/Ctrl+Shift+D` duplicate, `Alt+↑/↓` move lines), **Parameterized Queries** (`:user_id`, `?`) with a pre-run value dialog & per-query value memory, draggable horizontal splitter + expand toggle, **Run at cursor / selection** (`Cmd/Ctrl+Enter`), **Run all statements** (`Cmd/Ctrl+Shift+Enter`) into independent **Result 1 / Result 2 / …** sub-tabs, SQL formatter, schema-aware **autocomplete popup** at the caret (`↑/↓`, `Enter`/`Tab`, `Esc`, `Ctrl+Space`), and safety warnings for unqualified `UPDATE`/`DELETE`, `DROP`, and `TRUNCATE`.
- **Searchable History & Saved Queries / Snippets**: filter execution history by SQL, server, schema, or status without overwriting active consoles; save reusable SQL snippets (`Cmd/Ctrl+Shift+S`) with **Global** or **per-server** scope, folders, and pin-to-top.
- **Result Grid, Foreign Key Navigation & Vertical Record View**: aligned sticky header, drag-resizable columns (double-click divider to auto-fit), **Foreign Key Navigation** (`Cmd+Click` or right-click outbound FK cells to jump to referenced rows + right-click *Find referencing rows* for inbound child tables), server-side `WHERE` filter, right-click **Quick Filter** (`=` / `!=`), **View row as list** vertical drawer (column filter, hide NULLs, Pretty JSON, inline edit, copy JSON), **Compare 2 Rows** diff modal, and **Copy column as `IN (...)`**.
- **DataGrip-style Inline Cell Editing**: edit cells directly inside table tabs and single-table `SELECT` console results (`Double-click`, `Enter`, `F2`, or type-to-edit). Tracks original column names (`org_name`), supports `NULL`/`DEFAULT`, lazy-loads >4 KiB truncated cells, commits atomically (using `SAVEPOINT` inside open transactions), saves directly with `Cmd/Ctrl+S` on non-PROD while requiring **SQL Preview** on `PROD`, and protects staged edits with an **Unsaved Changes** dialog.
- **Copy, Streaming Export & CSV/JSON Import**: select rows with click, `Cmd/Ctrl+click`, `Shift+click`, or `Cmd/Ctrl+A`; copy as TSV (`Cmd/Ctrl+C`), CSV, JSON, JSON Lines, or SQL `INSERT`. Export the current page or **stream entire tables / full query results** directly to disk with constant RAM, live progress, and Cancel support. Import **CSV / TSV / JSON / JSONL** files into any table via right-click with auto column mapping, full-file **Dry-Run validation**, and atomic batched transactional inserts.
- **Table Structure Editor, Schema Compare & ER Diagram**: edit table columns, indexes, and foreign keys with live `ALTER TABLE` SQL preview; compare two schemas across servers (e.g., DEV vs STAGING) to inspect DDL diffs and generate migration scripts; explore interactive pan/zoom **ER Diagrams** per schema.
- **Diagnostics**: MySQL `EXPLAIN` / `EXPLAIN ANALYZE`, SQL Server SHOWPLAN / STATISTICS XML, live processlist with kill, and server health metrics.
- **Reliable Workspace Auto-Save & Sessions**: named workspace sessions automatically persist open console tabs, SQL drafts, editor layout, and connected servers (400 ms debounce, on query run, on focus loss, and on quit) and auto-reconnect on launch. Reopen accidentally closed tabs with `Cmd/Ctrl+Shift+T`.
- **Memory Guardrail**: all result grids share a 32 MiB budget with least-recently-used eviction.

### Keyboard shortcuts

| Action | macOS | Windows / Linux |
|---|---|---|
| Run statement at cursor (or selected SQL) | `Cmd+Enter` | `Ctrl+Enter` |
| Run all statements into separate result tabs | `Cmd+Shift+Enter` | `Ctrl+Shift+Enter` |
| Open autocomplete / accept / dismiss | `Ctrl+Space` / `Enter` or `Tab` / `Esc` | `Ctrl+Space` / `Enter` or `Tab` / `Esc` |
| New SQL console tab | `Cmd+T` | `Ctrl+T` |
| Reopen last closed tab | `Cmd+Shift+T` | `Ctrl+Shift+T` |
| Close active tab | `Cmd+W` | `Ctrl+W` |
| Switch to server 1..10 | `Cmd+1..0` | `Ctrl+1..0` |
| Find / Find & Replace in SQL editor | `Cmd+F` / `Cmd+H` | `Ctrl+F` / `Ctrl+H` |
| Toggle line comment in SQL editor | `Cmd+/` | `Ctrl+/` |
| Duplicate line / selection in SQL editor | `Cmd+Shift+D` | `Ctrl+Shift+D` |
| Move line / selection up or down | `Alt+↑` / `Alt+↓` | `Alt+↑` / `Alt+↓` |
| Save active SQL as snippet | `Cmd+Shift+S` | `Ctrl+Shift+S` |
| Save staged grid edits (Preview on PROD) | `Cmd+S` | `Ctrl+S` |
| Save inside New/Edit Connection modal | `Cmd+Enter` | `Ctrl+Enter` |
| Start inline cell edit | `Double-click` / `Enter` / `F2` / type | `Double-click` / `Enter` / `F2` / type |
| Open multiline cell Value Inspector | `Shift+Enter` | `Shift+Enter` |
| Set selected cell to SQL `NULL` | `Alt+Cmd+N` | `Alt+Ctrl+N` |
| Revert staged edit on selected cell | `Cmd+Z` | `Ctrl+Z` |
| Clone selected row | `Cmd+D` | `Ctrl+D` |
| Jump to referenced Foreign Key row | `Cmd+Click` on FK cell | `Ctrl+Click` on FK cell |
| Select all rows in the result table | `Cmd+A` | `Ctrl+A` |
| Copy selected rows as TSV | `Cmd+C` | `Ctrl+C` |
| Dismiss modal / cancel inline edit / clear selection | `Esc` | `Esc` |

### Known limitations

- SQL Server: Windows/AD authentication, named instances (`host\INSTANCE`), and Azure AD sign-in are not supported yet. Stopping a running query ends the whole SQL Server session, which reconnects automatically.
- Highlighting and completion follow MySQL rules on SQL Server connections.
- `cottondata-cli` connects to MySQL only.

---

## Configuration and data

| Item | macOS | Linux | Windows |
|---|---|---|---|
| Data directory | `~/Library/Application Support/CottonData` | `$XDG_CONFIG_HOME/cottondata` or `~/.config/cottondata` | `%APPDATA%\CottonData` |
| Saved servers | `servers.json` in the data directory | same | same |
| Workspace sessions & open consoles | `sessions.json` in the data directory | same | same |
| Query history (last 300 queries) | `history.json` in the data directory | same | same |
| Saved queries / snippets | `snippets.json` in the data directory | same | same |
| Passwords | macOS Keychain (optional, chosen per server) | kept in memory for the session | kept in memory for the session |

| Environment variable | Used by | Purpose |
|---|---|---|
| `COTTONDATA_PASSWORD` | `cottondata-cli` | Password for `test`, `schemas`, and `query` |
| `COTTONDATA_MSSQL_HOST`, `COTTONDATA_MSSQL_PORT`, `COTTONDATA_MSSQL_USER`, `COTTONDATA_MSSQL_PASSWORD`, `COTTONDATA_MSSQL_DATABASE` | `cargo test -p cottondata-mssql --test live_server` | Optional read-only test against a real SQL Server; skipped when unset |

---

## CLI

```bash
cottondata-cli doctor
cottondata-cli servers add local-db 127.0.0.1:3306 root myapp
cottondata-cli servers list
COTTONDATA_PASSWORD=secret cottondata-cli test local-db
COTTONDATA_PASSWORD=secret cottondata-cli query local-db "SELECT NOW()" --format csv
```

Full usage: `cottondata-cli help`.

---

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release -p cottondata-desktop -p cottondata-cli
```

| Platform | Package command | Output in `dist/` |
|---|---|---|
| macOS | `./scripts/bundle-macos.sh` | `CottonData.app`, `CottonData-macos-<arch>.zip`, `.dmg` |
| Linux | `./scripts/bundle-linux.sh` | `CottonData-linux-<arch>.tar.gz`, `.deb` when `dpkg-deb` exists |
| Windows | `.\scripts\bundle-windows.ps1` | `CottonData-windows-x64.zip` |

The package version comes from `version` in the root `Cargo.toml`. To bundle locally and publish a GitHub Release with `gh`, run `./scripts/publish-release.sh`. See [CHANGELOG.md](CHANGELOG.md) for release history.

### Workspace crates

```text
crates/
├── core/        # Server registry and limits, engines and SQL dialects, memory budget, error diagnostics
├── driver/      # DatabaseSession trait, shared result and catalog types, SSH tunnel
├── mysql/       # MySQL driver (pure Rust, rustls) and MySQL introspection
├── mssql/       # SQL Server driver (tiberius, rustls) and sys-catalog introspection
├── introspect/  # Driver-agnostic schema catalog with string interning
├── sql/         # Lexer, statement splitter, safety linter, completion, formatter
├── grid/        # Columnar result pages, staged edits, CSV/TSV/JSON/SQL export
├── storage/     # Server profiles, sessions, query history, macOS Keychain
├── cli/         # cottondata-cli
└── desktop/     # CottonData GUI (eframe/egui)
```

## License

Dual-licensed under **MIT** or **Apache-2.0**.

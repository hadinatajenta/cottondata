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
# CottonData CLI v0.2.0
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

- **MySQL and SQL Server** in one app. MySQL 5.7 / 8.x / 9.x and SQL Server 2012+ with SQL login.
- **Configurable server list**: 1 to 64 saved servers (default 10) from Settings. Saved servers cost almost no memory; only connected servers use RAM. `Cmd/Ctrl+1..0` switches the first ten.
- **Connections**: TCP, Unix socket (MySQL), TLS modes (Disabled, Preferred, Required, Verify CA, Verify Identity), SSH tunnel, environment badges (LOCAL / DEV / STAGING / PROD), and a Read-Only guard per server.
- **Schema navigator**: lazy loading of schemas or databases, tables, views, columns with type icons, and DDL, indexes, and foreign keys on demand.
- **SQL console**: highlighting, run statement at cursor (`Cmd/Ctrl+Enter`), formatter, schema-aware completion, and warnings for `UPDATE`/`DELETE` without `WHERE`, `DROP`, and `TRUNCATE`.
- **Result table**: aligned sticky header, two-axis scrolling, auto-sized columns, server-side `WHERE` filter, sorting, paging, and row counts.
- **Copy and export**: select rows with click, Cmd-click, Shift-click, or `Cmd/Ctrl+A`; copy with `Cmd/Ctrl+C` (TSV) or right-click as CSV, JSON, JSON Lines, or SQL `INSERT`. Export whole pages to a file in the same formats.
- **Inline editing**: stage inserts, clones, updates, and deletes, preview the SQL, and commit atomically.
- **Diagnostics**: MySQL `EXPLAIN` / `EXPLAIN ANALYZE`, SQL Server SHOWPLAN / STATISTICS XML, live processlist with kill, and health metrics.
- **Workspace sessions**: open tabs and connected servers are restored on launch.
- **Memory guardrail**: all result grids share a 32 MiB budget with least-recently-used eviction.

### Keyboard shortcuts

| Action | macOS | Windows / Linux |
|---|---|---|
| Run statement at cursor | `Cmd+Enter` | `Ctrl+Enter` |
| Switch to server 1..10 | `Cmd+1..0` | `Ctrl+1..0` |
| Select all rows in the result table | `Cmd+A` | `Ctrl+A` |
| Copy selected rows as TSV | `Cmd+C` | `Ctrl+C` |
| Open the cell inspector | Double-click a cell | Double-click a cell |
| Clear row selection | `Esc` | `Esc` |

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
cargo clippy --workspace --all-targets
cargo build --release -p cottondata-desktop -p cottondata-cli
```

| Platform | Package command | Output in `dist/` |
|---|---|---|
| macOS | `./scripts/bundle-macos.sh` | `CottonData.app`, `CottonData-macos-<arch>.zip`, `.dmg` |
| Linux | `./scripts/bundle-linux.sh` | `CottonData-linux-<arch>.tar.gz`, `.deb` when `dpkg-deb` exists |
| Windows | `.\scripts\bundle-windows.ps1` | `CottonData-windows-x64.zip` |

The package version comes from `version` in the root `Cargo.toml`.

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

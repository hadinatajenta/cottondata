use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GITHUB_RELEASES_API: &str =
    "https://api.github.com/repos/hadinatajenta/cottondata/releases/latest";
pub const GITHUB_RELEASES_PAGE: &str = "https://github.com/hadinatajenta/cottondata/releases";

const CHECKSUMS_ASSET_NAME: &str = "SHA256SUMS.txt";
const DOWNLOAD_CHUNK_BYTES: usize = 64 * 1024;
const MAX_CHECKSUMS_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateInstallMode {
    MacOsAppBundle { app_path: PathBuf },
    WindowsInnoSetup { installer_dir: PathBuf },
    WindowsPortable { exe_path: PathBuf },
    LinuxWritableBinary { exe_path: PathBuf },
    LinuxSystemPackage { deb_arch: &'static str },
    ManualOnly { reason: String },
}

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub version: String,
    pub release_notes: String,
    pub html_url: String,
    pub published_at: String,
    pub asset_name: Option<String>,
    pub asset_url: Option<String>,
    pub asset_size: u64,
    pub checksums_url: Option<String>,
    pub is_newer: bool,
    pub install_mode: UpdateInstallMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyOutcome {
    Relaunching,
    DownloadedPackage {
        path: PathBuf,
        install_command: String,
    },
}

struct GitHubRelease {
    tag_name: String,
    body: String,
    html_url: String,
    published_at: String,
    assets: Vec<GitHubAsset>,
}

struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

fn parse_github_release_json(body: &str) -> Result<GitHubRelease, String> {
    let root: Value = serde_json::from_str(body)
        .map_err(|e| format!("Could not parse GitHub release metadata: {e}"))?;
    let tag_name = root
        .get("tag_name")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "GitHub release metadata is missing tag_name.".to_string())?
        .to_string();
    let release_notes = root
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let html_url = root
        .get("html_url")
        .and_then(Value::as_str)
        .filter(|u| !u.trim().is_empty())
        .unwrap_or(GITHUB_RELEASES_PAGE)
        .to_string();
    let published_at = root
        .get("published_at")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut assets = Vec::new();
    if let Some(arr) = root.get("assets").and_then(Value::as_array) {
        for item in arr {
            let (Some(name), Some(url)) = (
                item.get("name").and_then(Value::as_str),
                item.get("browser_download_url").and_then(Value::as_str),
            ) else {
                continue;
            };
            assets.push(GitHubAsset {
                name: name.to_string(),
                browser_download_url: url.to_string(),
                size: item.get("size").and_then(Value::as_u64).unwrap_or(0),
            });
        }
    }
    Ok(GitHubRelease {
        tag_name,
        body: release_notes,
        html_url,
        published_at,
        assets,
    })
}

type SemverParts<'a> = ((u64, u64, u64), Option<&'a str>);

#[cfg(test)]
pub fn parse_semver(s: &str) -> Option<(u64, u64, u64)> {
    parse_semver_parts(s).map(|(core, _)| core)
}

fn parse_semver_parts(s: &str) -> Option<SemverParts<'_>> {
    let trimmed = s.trim();
    let without_v = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    let without_build = without_v.split('+').next()?;
    let (core_str, prerelease) = match without_build.split_once('-') {
        Some((core, pre)) if !pre.is_empty() => (core, Some(pre)),
        Some((core, _)) => (core, None),
        None => (without_build, None),
    };
    let mut parts = core_str.split('.');
    let major = parse_numeric_part(parts.next()?)?;
    let minor = parse_numeric_part(parts.next()?)?;
    let patch = parse_numeric_part(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some(((major, minor, patch), prerelease))
}

fn parse_numeric_part(part: &str) -> Option<u64> {
    if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse::<u64>().ok()
}

fn compare_prerelease(a: &str, b: &str) -> std::cmp::Ordering {
    let mut a_parts = a.split('.');
    let mut b_parts = b.split('.');
    loop {
        match (a_parts.next(), b_parts.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(ap), Some(bp)) => {
                let ord = match (parse_numeric_part(ap), parse_numeric_part(bp)) {
                    (Some(an), Some(bn)) => an.cmp(&bn),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => ap.cmp(bp),
                };
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    let (Some((cur_core, cur_pre)), Some((cand_core, cand_pre))) =
        (parse_semver_parts(current), parse_semver_parts(candidate))
    else {
        return false;
    };
    if cand_core != cur_core {
        return cand_core > cur_core;
    }
    match (cur_pre, cand_pre) {
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => false,
        (Some(cur_p), Some(cand_p)) => compare_prerelease(cand_p, cur_p).is_gt(),
    }
}

pub fn detect_current_install_mode() -> UpdateInstallMode {
    match std::env::current_exe() {
        Ok(exe) => detect_install_mode(&exe),
        Err(err) => UpdateInstallMode::ManualOnly {
            reason: format!("Could not resolve current executable path: {err}"),
        },
    }
}

pub fn detect_install_mode(current_exe: &Path) -> UpdateInstallMode {
    detect_install_mode_for_platform(
        current_exe,
        std::env::consts::OS,
        std::env::consts::ARCH,
        is_dir_writable,
        |p| p.is_file(),
    )
}

fn is_cargo_dev_build(path: &Path) -> bool {
    let names: Vec<&str> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(os) => os.to_str(),
            _ => None,
        })
        .collect();
    for (idx, &comp) in names.iter().enumerate() {
        if comp == "target" {
            for offset in 1..=2 {
                if let Some(&next) = names.get(idx + offset) {
                    if matches!(next, "debug" | "release") {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn find_macos_app_bundle(current_exe: &Path) -> Option<PathBuf> {
    let macos_dir = current_exe.parent()?;
    let contents_dir = macos_dir.parent()?;
    let app_dir = contents_dir.parent()?;
    let is_macos = macos_dir.file_name().is_some_and(|n| n == "MacOS");
    let is_contents = contents_dir.file_name().is_some_and(|n| n == "Contents");
    let is_app = app_dir.extension().is_some_and(|ext| ext == "app");
    if is_macos && is_contents && is_app {
        Some(app_dir.to_path_buf())
    } else {
        None
    }
}

fn detect_install_mode_for_platform<W, F>(
    current_exe: &Path,
    os: &str,
    arch: &str,
    dir_is_writable: W,
    file_exists: F,
) -> UpdateInstallMode
where
    W: Fn(&Path) -> bool,
    F: Fn(&Path) -> bool,
{
    if is_cargo_dev_build(current_exe) {
        return UpdateInstallMode::ManualOnly {
            reason: "Running from a Cargo build directory; update manually or build a release bundle.".into(),
        };
    }

    match os {
        "macos" => {
            let path_str = current_exe.to_string_lossy();
            if path_str.contains("AppTranslocation") {
                return UpdateInstallMode::ManualOnly {
                    reason: "CottonData is running from macOS App Translocation. Move CottonData.app to /Applications to enable automatic updates.".into(),
                };
            }
            let Some(app_path) = find_macos_app_bundle(current_exe) else {
                return UpdateInstallMode::ManualOnly {
                    reason: "Not running from a CottonData.app bundle; download the latest release from GitHub.".into(),
                };
            };
            let Some(parent) = app_path.parent() else {
                return UpdateInstallMode::ManualOnly {
                    reason: "Unable to determine parent directory of CottonData.app.".into(),
                };
            };
            if dir_is_writable(parent) {
                UpdateInstallMode::MacOsAppBundle { app_path }
            } else {
                UpdateInstallMode::ManualOnly {
                    reason: format!(
                        "{} is not writable by the current user; download the update from GitHub.",
                        parent.display()
                    ),
                }
            }
        }
        "windows" => {
            let Some(parent) = current_exe.parent() else {
                return UpdateInstallMode::ManualOnly {
                    reason: "Unable to determine installation directory.".into(),
                };
            };
            if file_exists(&parent.join("unins000.exe")) {
                UpdateInstallMode::WindowsInnoSetup {
                    installer_dir: parent.to_path_buf(),
                }
            } else if dir_is_writable(parent) {
                UpdateInstallMode::WindowsPortable {
                    exe_path: current_exe.to_path_buf(),
                }
            } else {
                UpdateInstallMode::ManualOnly {
                    reason: format!(
                        "{} is not writable; download the installer from GitHub Releases.",
                        parent.display()
                    ),
                }
            }
        }
        "linux" => {
            let Some(parent) = current_exe.parent() else {
                return UpdateInstallMode::ManualOnly {
                    reason: "Unable to determine executable directory.".into(),
                };
            };
            if dir_is_writable(parent) {
                return UpdateInstallMode::LinuxWritableBinary {
                    exe_path: current_exe.to_path_buf(),
                };
            }
            let is_system_dir = current_exe.starts_with("/usr/bin")
                || current_exe.starts_with("/usr/local/bin")
                || current_exe.starts_with("/opt");
            if is_system_dir {
                if let Some(deb_arch) = linux_deb_arch(arch) {
                    return UpdateInstallMode::LinuxSystemPackage { deb_arch };
                }
            }
            UpdateInstallMode::ManualOnly {
                reason: format!(
                    "{} is not writable; download the latest package from GitHub Releases.",
                    parent.display()
                ),
            }
        }
        _ => UpdateInstallMode::ManualOnly {
            reason: format!("Automatic updates are not supported on {os}/{arch}."),
        },
    }
}

fn linux_deb_arch(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" | "x64" | "amd64" => Some("amd64"),
        "aarch64" | "arm64" => Some("arm64"),
        _ => None,
    }
}

fn is_dir_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!(".cottondata-write-probe-{}", std::process::id()));
    match File::create(&probe) {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub fn expected_asset_name(
    version: &str,
    os: &str,
    arch: &str,
    install_mode: &UpdateInstallMode,
) -> Option<String> {
    let clean_version = version
        .trim()
        .trim_start_matches(['v', 'V']);
    let arch_label = match arch {
        "aarch64" | "arm64" => "arm64",
        "x86_64" | "x64" | "amd64" => "x64",
        _ => return None,
    };

    match (os, install_mode) {
        ("macos", UpdateInstallMode::MacOsAppBundle { .. }) => {
            Some(format!("CottonData-macos-{arch_label}.zip"))
        }
        ("windows", UpdateInstallMode::WindowsInnoSetup { .. }) if arch_label == "x64" => {
            Some("CottonData-setup-windows-x64.exe".to_string())
        }
        ("windows", UpdateInstallMode::WindowsPortable { .. }) if arch_label == "x64" => {
            Some("CottonData-windows-x64.zip".to_string())
        }
        ("linux", UpdateInstallMode::LinuxWritableBinary { .. }) => {
            Some(format!("CottonData-linux-{arch_label}.tar.gz"))
        }
        ("linux", UpdateInstallMode::LinuxSystemPackage { deb_arch }) => {
            if clean_version.is_empty() {
                None
            } else {
                Some(format!("cottondata_{clean_version}_{deb_arch}.deb"))
            }
        }
        _ => None,
    }
}

pub fn parse_sha256sums(contents: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(ws_idx) = line.find([' ', '\t']) else {
            continue;
        };
        let hash = &line[..ws_idx];
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let rest = line[ws_idx..].trim_start_matches([' ', '\t']);
        let rest = rest.strip_prefix('*').unwrap_or(rest);
        let filename = rest.strip_prefix("./").unwrap_or(rest).trim();
        if !filename.is_empty() {
            map.insert(filename.to_string(), hash.to_ascii_lowercase());
        }
    }
    map
}

#[cfg(test)]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    encode_hex(&hasher.finalize())
}

#[cfg(test)]
pub fn verify_sha256_bytes(bytes: &[u8], expected_hex: &str) -> Result<(), String> {
    let expected = expected_hex.trim().to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid expected SHA-256 digest.".into());
    }
    let actual = sha256_hex(bytes);
    if actual != expected {
        return Err(format!(
            "SHA-256 checksum mismatch (expected {expected}, got {actual})."
        ));
    }
    Ok(())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

fn format_ureq_error(err: ureq::Error) -> String {
    match err {
        ureq::Error::StatusCode(403) => {
            "GitHub API rate limit reached (HTTP 403). Try again later or open GitHub Releases.".into()
        }
        ureq::Error::StatusCode(404) => {
            "No published release was found on GitHub (HTTP 404).".into()
        }
        ureq::Error::StatusCode(code) => format!("GitHub returned HTTP {code}."),
        ureq::Error::Timeout(_) => "Network request timed out while contacting GitHub.".into(),
        other => format!("Network error: {other}"),
    }
}

fn user_agent() -> String {
    format!("CottonData/{CURRENT_VERSION}")
}

pub fn check_latest_release() -> Result<ReleaseInfo, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .https_only(true)
        .build()
        .into();

    let body = agent
        .get(GITHUB_RELEASES_API)
        .header("User-Agent", &user_agent())
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(format_ureq_error)?
        .body_mut()
        .read_to_string()
        .map_err(format_ureq_error)?;

    let release = parse_github_release_json(&body)?;

    let install_mode = detect_current_install_mode();
    let version = release
        .tag_name
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_string();
    let expected_name = expected_asset_name(
        &version,
        std::env::consts::OS,
        std::env::consts::ARCH,
        &install_mode,
    );

    let matched_asset = expected_name
        .as_deref()
        .and_then(|name| release.assets.iter().find(|a| a.name == name));
    let checksums_url = release
        .assets
        .iter()
        .find(|a| a.name == CHECKSUMS_ASSET_NAME)
        .map(|a| a.browser_download_url.clone());

    let is_newer = is_newer_version(CURRENT_VERSION, &version);

    Ok(ReleaseInfo {
        tag_name: release.tag_name,
        version,
        release_notes: release.body,
        html_url: release.html_url,
        published_at: release.published_at,
        asset_name: matched_asset.map(|a| a.name.clone()),
        asset_url: matched_asset.map(|a| a.browser_download_url.clone()),
        asset_size: matched_asset.map_or(0, |a| a.size),
        checksums_url,
        is_newer,
        install_mode,
    })
}

fn sanitize_path_segment(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('.');
    if trimmed.is_empty() {
        "latest".to_string()
    } else {
        trimmed.to_string()
    }
}

fn is_safe_filename(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
        && name != "."
}

pub fn download_and_verify_asset<F>(
    info: &ReleaseInfo,
    cancel: &AtomicBool,
    mut on_progress: F,
) -> Result<PathBuf, String>
where
    F: FnMut(u64, u64),
{
    if cancel.load(Ordering::Relaxed) {
        return Err("Update download cancelled.".into());
    }
    let (Some(asset_name), Some(asset_url), Some(checksums_url)) = (
        info.asset_name.as_deref(),
        info.asset_url.as_deref(),
        info.checksums_url.as_deref(),
    ) else {
        return Err(
            "Release is missing a verified package or SHA256SUMS.txt; download manually from GitHub Releases."
                .into(),
        );
    };
    if !is_safe_filename(asset_name) {
        return Err(format!("Refusing unsafe release asset name: {asset_name}"));
    }
    if !asset_url.starts_with("https://") || !checksums_url.starts_with("https://") {
        return Err("Refusing non-HTTPS update URL.".into());
    }

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_per_read(Some(Duration::from_secs(30)))
        .build()
        .into();

    let checksums_text = agent
        .get(checksums_url)
        .header("User-Agent", &user_agent())
        .call()
        .map_err(format_ureq_error)?
        .body_mut()
        .with_config()
        .limit(MAX_CHECKSUMS_BYTES)
        .read_to_string()
        .map_err(format_ureq_error)?;

    let checksums = parse_sha256sums(&checksums_text);
    let Some(expected_hash) = checksums.get(asset_name) else {
        return Err(format!(
            "SHA256SUMS.txt does not contain a checksum entry for {asset_name}; refusing unverified install."
        ));
    };

    if cancel.load(Ordering::Relaxed) {
        return Err("Update download cancelled.".into());
    }

    let temp_dir = std::env::temp_dir().join(format!(
        "cottondata-update-{}",
        sanitize_path_segment(&info.version)
    ));
    fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("Could not create temp update directory: {e}"))?;
    let dest_path = temp_dir.join(asset_name);

    let mut response = agent
        .get(asset_url)
        .header("User-Agent", &user_agent())
        .call()
        .map_err(format_ureq_error)?;

    let total_size = response.body().content_length().unwrap_or(info.asset_size);
    let mut reader = response.body_mut().as_reader();
    let mut file = File::create(&dest_path)
        .map_err(|e| format!("Could not create download file {}: {e}", dest_path.display()))?;

    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; DOWNLOAD_CHUNK_BYTES];
    let mut downloaded = 0u64;
    on_progress(0, total_size);

    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = fs::remove_file(&dest_path);
            return Err("Update download cancelled.".into());
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) => {
                drop(file);
                let _ = fs::remove_file(&dest_path);
                return Err(format!("Download interrupted: {err}"));
            }
        };
        if let Err(err) = file.write_all(&buf[..n]) {
            drop(file);
            let _ = fs::remove_file(&dest_path);
            return Err(format!("Failed writing update file: {err}"));
        }
        hasher.update(&buf[..n]);
        downloaded = downloaded.saturating_add(n as u64);
        on_progress(downloaded, total_size);
    }

    if let Err(err) = file.flush() {
        drop(file);
        let _ = fs::remove_file(&dest_path);
        return Err(format!("Failed flushing update file: {err}"));
    }
    drop(file);

    let actual_hash = encode_hex(&hasher.finalize());
    if &actual_hash != expected_hash {
        let _ = fs::remove_file(&dest_path);
        return Err(format!(
            "SHA-256 verification failed for {asset_name} (expected {expected_hash}, got {actual_hash})."
        ));
    }

    Ok(dest_path)
}

pub fn apply_update_and_relaunch(
    downloaded_file: &Path,
    info: &ReleaseInfo,
) -> Result<ApplyOutcome, String> {
    if !downloaded_file.is_file() {
        return Err(format!(
            "Downloaded update file not found: {}",
            downloaded_file.display()
        ));
    }

    match &info.install_mode {
        UpdateInstallMode::MacOsAppBundle { app_path } => {
            apply_macos_bundle_update(downloaded_file, app_path)
        }
        UpdateInstallMode::WindowsInnoSetup { .. } => apply_windows_inno_update(downloaded_file),
        UpdateInstallMode::WindowsPortable { exe_path } => {
            apply_windows_portable_update(downloaded_file, exe_path)
        }
        UpdateInstallMode::LinuxWritableBinary { exe_path } => {
            apply_linux_writable_update(downloaded_file, exe_path)
        }
        UpdateInstallMode::LinuxSystemPackage { .. } => {
            apply_linux_deb_handoff(downloaded_file)
        }
        UpdateInstallMode::ManualOnly { .. } => {
            Err("Automatic install is not available in this build; download from GitHub Releases.".into())
        }
    }
}

fn apply_macos_bundle_update(zip_path: &Path, app_path: &Path) -> Result<ApplyOutcome, String> {
    let temp_root = zip_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let extract_dir = temp_root.join("extracted");
    let _ = fs::remove_dir_all(&extract_dir);
    fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("Could not create extraction directory: {e}"))?;

    let ditto_out = Command::new("ditto")
        .arg("-x")
        .arg("-k")
        .arg(zip_path)
        .arg(&extract_dir)
        .output()
        .map_err(|e| format!("Failed to invoke ditto to extract update: {e}"))?;
    if !ditto_out.status.success() {
        let stderr = String::from_utf8_lossy(&ditto_out.stderr);
        return Err(format!("Failed to extract macOS update archive: {stderr}"));
    }

    let new_app = locate_app_bundle(&extract_dir)
        .ok_or_else(|| "Extracted archive does not contain CottonData.app.".to_string())?;

    let _ = Command::new("xattr")
        .arg("-dr")
        .arg("com.apple.quarantine")
        .arg(&new_app)
        .output();

    let codesign_out = Command::new("codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(&new_app)
        .output()
        .map_err(|e| format!("Failed to invoke codesign verification: {e}"))?;
    if !codesign_out.status.success() {
        let stderr = String::from_utf8_lossy(&codesign_out.stderr);
        return Err(format!(
            "Code signature verification failed for new CottonData.app: {}",
            stderr.trim()
        ));
    }

    const SWAP_SCRIPT: &str = r#"
PID="$1"
APP="$2"
NEW_APP="$3"
TEMP_DIR="$4"
OLD_APP="${APP}.old"
i=0
while kill -0 "$PID" 2>/dev/null && [ "$i" -lt 150 ]; do
    sleep 0.1
    i=$((i + 1))
done
rm -rf "$OLD_APP"
if ! mv "$APP" "$OLD_APP"; then
    exit 1
fi
if ! mv "$NEW_APP" "$APP"; then
    mv "$OLD_APP" "$APP" 2>/dev/null || true
    exit 1
fi
rm -rf "$OLD_APP" "$TEMP_DIR"
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$APP" >/dev/null 2>&1 || true
open -n "$APP"
"#;

    Command::new("/bin/sh")
        .arg("-c")
        .arg(SWAP_SCRIPT)
        .arg("cottondata-updater")
        .arg(std::process::id().to_string())
        .arg(app_path)
        .arg(&new_app)
        .arg(&temp_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to launch macOS update helper: {e}"))?;

    Ok(ApplyOutcome::Relaunching)
}

fn locate_app_bundle(extract_dir: &Path) -> Option<PathBuf> {
    let direct = extract_dir.join("CottonData.app");
    if direct.is_dir() {
        return Some(direct);
    }
    let entries = fs::read_dir(extract_dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.extension().is_some_and(|e| e == "app") {
            return Some(path);
        }
        if path.is_dir() {
            let nested = path.join("CottonData.app");
            if nested.is_dir() {
                return Some(nested);
            }
        }
    }
    None
}

fn apply_windows_inno_update(installer_path: &Path) -> Result<ApplyOutcome, String> {
    Command::new(installer_path)
        .args(["/SILENT", "/CLOSEAPPLICATIONS", "/RESTARTAPPLICATIONS"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to launch Windows installer: {e}"))?;
    Ok(ApplyOutcome::Relaunching)
}

#[cfg(windows)]
fn apply_windows_portable_update(zip_path: &Path, exe_path: &Path) -> Result<ApplyOutcome, String> {
    let temp_root = zip_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let extract_dir = temp_root.join("extracted");
    let _ = fs::remove_dir_all(&extract_dir);
    fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("Could not create extraction directory: {e}"))?;

    let archive_file = File::open(zip_path)
        .map_err(|e| format!("Could not open downloaded zip archive: {e}"))?;
    let mut archive = zip::ZipArchive::new(archive_file)
        .map_err(|e| format!("Invalid Windows zip archive: {e}"))?;

    let mut extracted_gui: Option<PathBuf> = None;
    let mut extracted_cli: Option<PathBuf> = None;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("Failed reading zip entry: {e}"))?;
        let Some(rel_path) = entry.enclosed_name() else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let out_path = extract_dir.join(&rel_path);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Could not create directory while extracting zip: {e}"))?;
        }
        let mut out_file = File::create(&out_path)
            .map_err(|e| format!("Could not write extracted file: {e}"))?;
        std::io::copy(&mut entry, &mut out_file)
            .map_err(|e| format!("Failed extracting zip entry: {e}"))?;
        if let Some(fname) = out_path.file_name().and_then(|s| s.to_str()) {
            if fname.eq_ignore_ascii_case("cottondata.exe") {
                extracted_gui = Some(out_path);
            } else if fname.eq_ignore_ascii_case("cottondata-cli.exe") {
                extracted_cli = Some(out_path);
            }
        }
    }

    let new_exe = extracted_gui
        .ok_or_else(|| "Downloaded zip does not contain CottonData.exe.".to_string())?;
    let install_dir = exe_path
        .parent()
        .ok_or_else(|| "Could not resolve executable parent directory.".to_string())?;

    let staged_exe = install_dir.join(".CottonData.exe.new");
    let old_exe = old_binary_path(exe_path);
    fs::copy(&new_exe, &staged_exe)
        .map_err(|e| format!("Failed staging updated executable: {e}"))?;
    let _ = fs::remove_file(&old_exe);
    fs::rename(exe_path, &old_exe).map_err(|e| {
        let _ = fs::remove_file(&staged_exe);
        format!("Could not move current executable aside: {e}")
    })?;
    if let Err(err) = fs::rename(&staged_exe, exe_path) {
        let _ = fs::rename(&old_exe, exe_path);
        let _ = fs::remove_file(&staged_exe);
        return Err(format!("Could not replace executable (rolled back): {err}"));
    }

    if let Some(new_cli) = extracted_cli {
        let cli_dst = install_dir.join("cottondata-cli.exe");
        let cli_staged = install_dir.join(".cottondata-cli.exe.new");
        if fs::copy(&new_cli, &cli_staged).is_ok() {
            let _ = fs::remove_file(&cli_dst);
            if fs::rename(&cli_staged, &cli_dst).is_err() {
                let _ = fs::remove_file(&cli_staged);
            }
        }
    }

    let _ = fs::remove_dir_all(&extract_dir);
    Command::new(exe_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Updated binary installed, but relaunch failed: {e}"))?;

    Ok(ApplyOutcome::Relaunching)
}

#[cfg(not(windows))]
fn apply_windows_portable_update(_zip_path: &Path, _exe_path: &Path) -> Result<ApplyOutcome, String> {
    Err("Windows portable update can only be applied on Windows.".into())
}

fn apply_linux_writable_update(tarball_path: &Path, exe_path: &Path) -> Result<ApplyOutcome, String> {
    let temp_root = tarball_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let extract_dir = temp_root.join("extracted");
    let _ = fs::remove_dir_all(&extract_dir);
    fs::create_dir_all(&extract_dir)
        .map_err(|e| format!("Could not create extraction directory: {e}"))?;

    let tar_out = Command::new("tar")
        .arg("-xzf")
        .arg(tarball_path)
        .arg("-C")
        .arg(&extract_dir)
        .output()
        .map_err(|e| format!("Failed to invoke tar to extract update: {e}"))?;
    if !tar_out.status.success() {
        let stderr = String::from_utf8_lossy(&tar_out.stderr);
        return Err(format!("Failed to extract Linux tarball: {stderr}"));
    }

    let (new_gui, new_cli) = locate_linux_binaries(&extract_dir);
    let new_gui = new_gui
        .ok_or_else(|| "Extracted archive does not contain the CottonData binary.".to_string())?;
    let install_dir = exe_path
        .parent()
        .ok_or_else(|| "Could not determine executable parent directory.".to_string())?;

    let staged = install_dir.join(".cottondata-update.tmp");
    fs::copy(&new_gui, &staged)
        .map_err(|e| format!("Failed staging updated binary into {}: {e}", install_dir.display()))?;
    set_executable_permissions(&staged)?;
    if let Err(err) = fs::rename(&staged, exe_path) {
        let _ = fs::remove_file(&staged);
        return Err(format!("Failed atomically replacing {}: {err}", exe_path.display()));
    }

    if let Some(cli_src) = new_cli {
        let cli_dst = install_dir.join("cottondata-cli");
        let cli_staged = install_dir.join(".cottondata-cli-update.tmp");
        if fs::copy(&cli_src, &cli_staged).is_ok() {
            let _ = set_executable_permissions(&cli_staged);
            if fs::rename(&cli_staged, &cli_dst).is_err() {
                let _ = fs::remove_file(&cli_staged);
            }
        }
    }

    let _ = fs::remove_dir_all(&extract_dir);
    Command::new(exe_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Updated binary installed, but relaunch failed: {e}"))?;

    Ok(ApplyOutcome::Relaunching)
}

fn locate_linux_binaries(root: &Path) -> (Option<PathBuf>, Option<PathBuf>) {
    let mut gui = None;
    let mut cli = None;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    if name.eq_ignore_ascii_case("cottondata") {
                        gui = Some(path);
                    } else if name.eq_ignore_ascii_case("cottondata-cli") {
                        cli = Some(path);
                    }
                }
            }
        }
    }
    (gui, cli)
}

#[cfg(unix)]
fn set_executable_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("Could not mark {} executable: {e}", path.display()))
}

#[cfg(not(unix))]
fn set_executable_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn apply_linux_deb_handoff(deb_path: &Path) -> Result<ApplyOutcome, String> {
    let file_name = deb_path
        .file_name()
        .ok_or_else(|| "Downloaded package path has no file name.".to_string())?;
    let target_dir = resolve_downloads_dir();
    let dest = if target_dir.is_dir() || fs::create_dir_all(&target_dir).is_ok() {
        let candidate = target_dir.join(file_name);
        if candidate == deb_path || fs::rename(deb_path, &candidate).is_ok() {
            candidate
        } else if fs::copy(deb_path, &candidate).is_ok() {
            let _ = fs::remove_file(deb_path);
            candidate
        } else {
            deb_path.to_path_buf()
        }
    } else {
        deb_path.to_path_buf()
    };

    let install_command = format!("sudo apt install {}", dest.display());
    Ok(ApplyOutcome::DownloadedPackage {
        path: dest,
        install_command,
    })
}

fn resolve_downloads_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DOWNLOAD_DIR") {
        let trimmed = xdg.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed).join("Downloads");
        }
    }
    std::env::temp_dir()
}

fn old_binary_path(exe_path: &Path) -> PathBuf {
    let mut os = exe_path.as_os_str().to_os_string();
    os.push(".old");
    PathBuf::from(os)
}

pub fn cleanup_stale_old_binaries() {
    let Ok(exe_path) = std::env::current_exe() else {
        return;
    };
    let old_exe = old_binary_path(&exe_path);
    if old_exe.is_file() {
        let _ = fs::remove_file(&old_exe);
    }
    if let Some(app_path) = find_macos_app_bundle(&exe_path) {
        let old_app = old_binary_path(&app_path);
        if old_app.is_dir() {
            let _ = fs::remove_dir_all(&old_app);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_semver_with_v_prefix_and_prerelease() {
        assert_eq!(parse_semver("0.11.0"), Some((0, 11, 0)));
        assert_eq!(parse_semver("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_semver("V2.10.4-rc.1+build.42"), Some((2, 10, 4)));
        assert_eq!(parse_semver("invalid"), None);
        assert_eq!(parse_semver("1.2"), None);
        assert_eq!(parse_semver("1.2.3.4"), None);
    }

    #[test]
    fn compares_versions_including_prerelease() {
        assert!(is_newer_version("0.10.0", "0.11.0"));
        assert!(is_newer_version("0.10.0", "v0.10.1"));
        assert!(is_newer_version("v0.10.9", "1.0.0"));
        assert!(!is_newer_version("0.11.0", "0.11.0"));
        assert!(!is_newer_version("0.11.0", "v0.10.9"));
        assert!(is_newer_version("0.11.0-rc.1", "0.11.0"));
        assert!(!is_newer_version("0.11.0", "0.11.0-rc.1"));
        assert!(is_newer_version("0.11.0-rc.1", "0.11.0-rc.2"));
        assert!(!is_newer_version("bad", "0.11.0"));
    }

    #[test]
    fn parses_sha256sums_formats() {
        let hash_a = "a".repeat(64);
        let hash_b = "B".repeat(64);
        let hash_c = "0123456789abcdef".repeat(4);
        let input = format!(
            "# release checksums\n\
             {hash_a}  CottonData-macos-arm64.zip\n\
             {hash_b} *CottonData-windows-x64.zip\n\
             {hash_c}  ./cottondata_0.11.0_amd64.deb\n\
             short  ignored.zip\n\
             not_hex_zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz  bad.zip\n\
             \n"
        );
        let parsed = parse_sha256sums(&input);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed.get("CottonData-macos-arm64.zip"), Some(&hash_a));
        assert_eq!(
            parsed.get("CottonData-windows-x64.zip"),
            Some(&"b".repeat(64))
        );
        assert_eq!(parsed.get("cottondata_0.11.0_amd64.deb"), Some(&hash_c));
    }

    #[test]
    fn verifies_sha256_pass_and_mismatch() {
        let payload = b"cottondata release payload";
        let digest = sha256_hex(payload);
        assert_eq!(digest.len(), 64);
        assert!(verify_sha256_bytes(payload, &digest).is_ok());
        assert!(verify_sha256_bytes(payload, &digest.to_uppercase()).is_ok());
        let wrong = "0".repeat(64);
        let err = verify_sha256_bytes(payload, &wrong).unwrap_err();
        assert!(err.contains("mismatch"));
    }

    #[test]
    fn resolves_expected_asset_names_across_platforms() {
        assert_eq!(
            expected_asset_name(
                "0.11.0",
                "macos",
                "aarch64",
                &UpdateInstallMode::MacOsAppBundle {
                    app_path: PathBuf::from("/Applications/CottonData.app")
                }
            ),
            Some("CottonData-macos-arm64.zip".into())
        );
        assert_eq!(
            expected_asset_name(
                "v0.11.0",
                "macos",
                "x86_64",
                &UpdateInstallMode::MacOsAppBundle {
                    app_path: PathBuf::from("/Applications/CottonData.app")
                }
            ),
            Some("CottonData-macos-x64.zip".into())
        );
        assert_eq!(
            expected_asset_name(
                "0.11.0",
                "windows",
                "x86_64",
                &UpdateInstallMode::WindowsInnoSetup {
                    installer_dir: PathBuf::from(r"C:\Program Files\CottonData")
                }
            ),
            Some("CottonData-setup-windows-x64.exe".into())
        );
        assert_eq!(
            expected_asset_name(
                "0.11.0",
                "windows",
                "x86_64",
                &UpdateInstallMode::WindowsPortable {
                    exe_path: PathBuf::from(r"D:\Tools\CottonData\CottonData.exe")
                }
            ),
            Some("CottonData-windows-x64.zip".into())
        );
        assert_eq!(
            expected_asset_name(
                "0.11.0",
                "linux",
                "x86_64",
                &UpdateInstallMode::LinuxWritableBinary {
                    exe_path: PathBuf::from("/home/user/.local/bin/CottonData")
                }
            ),
            Some("CottonData-linux-x64.tar.gz".into())
        );
        assert_eq!(
            expected_asset_name(
                "0.11.0",
                "linux",
                "aarch64",
                &UpdateInstallMode::LinuxWritableBinary {
                    exe_path: PathBuf::from("/home/user/.local/bin/CottonData")
                }
            ),
            Some("CottonData-linux-arm64.tar.gz".into())
        );
        assert_eq!(
            expected_asset_name(
                "v0.11.0",
                "linux",
                "x86_64",
                &UpdateInstallMode::LinuxSystemPackage { deb_arch: "amd64" }
            ),
            Some("cottondata_0.11.0_amd64.deb".into())
        );
        assert_eq!(
            expected_asset_name(
                "0.11.0",
                "linux",
                "aarch64",
                &UpdateInstallMode::LinuxSystemPackage { deb_arch: "arm64" }
            ),
            Some("cottondata_0.11.0_arm64.deb".into())
        );
        assert_eq!(
            expected_asset_name(
                "0.11.0",
                "macos",
                "aarch64",
                &UpdateInstallMode::ManualOnly {
                    reason: "dev".into()
                }
            ),
            None
        );
    }

    #[test]
    fn detects_dev_build_and_platform_install_modes() {
        let dev_debug = Path::new("/Users/dev/cottondata/target/debug/CottonData");
        assert!(matches!(
            detect_install_mode(dev_debug),
            UpdateInstallMode::ManualOnly { .. }
        ));
        let dev_target_release =
            Path::new("/Users/dev/cottondata/target/aarch64-apple-darwin/release/CottonData");
        assert!(matches!(
            detect_install_mode(dev_target_release),
            UpdateInstallMode::ManualOnly { .. }
        ));

        let mac_app = Path::new("/Applications/CottonData.app/Contents/MacOS/CottonData");
        assert_eq!(
            detect_install_mode_for_platform(mac_app, "macos", "aarch64", |_| true, |_| false),
            UpdateInstallMode::MacOsAppBundle {
                app_path: PathBuf::from("/Applications/CottonData.app")
            }
        );

        let translocated = Path::new(
            "/private/var/folders/xx/yy/T/AppTranslocation/ABC/d/CottonData.app/Contents/MacOS/CottonData",
        );
        assert!(matches!(
            detect_install_mode_for_platform(translocated, "macos", "aarch64", |_| true, |_| false),
            UpdateInstallMode::ManualOnly { .. }
        ));

        let win_exe = Path::new("/opt/CottonData/CottonData.exe");
        assert_eq!(
            detect_install_mode_for_platform(
                win_exe,
                "windows",
                "x86_64",
                |_| true,
                |p| p.file_name().is_some_and(|n| n == "unins000.exe")
            ),
            UpdateInstallMode::WindowsInnoSetup {
                installer_dir: PathBuf::from("/opt/CottonData")
            }
        );
        assert_eq!(
            detect_install_mode_for_platform(win_exe, "windows", "x86_64", |_| true, |_| false),
            UpdateInstallMode::WindowsPortable {
                exe_path: win_exe.to_path_buf()
            }
        );

        let linux_sys = Path::new("/usr/bin/CottonData");
        assert_eq!(
            detect_install_mode_for_platform(linux_sys, "linux", "x86_64", |_| false, |_| false),
            UpdateInstallMode::LinuxSystemPackage { deb_arch: "amd64" }
        );
        let linux_local = Path::new("/home/user/.local/bin/CottonData");
        assert_eq!(
            detect_install_mode_for_platform(linux_local, "linux", "aarch64", |_| true, |_| false),
            UpdateInstallMode::LinuxWritableBinary {
                exe_path: linux_local.to_path_buf()
            }
        );
    }

    #[test]
    fn parses_github_release_payload() {
        let payload = r###"{
            "tag_name": "v0.11.0",
            "body": "## Highlights\n- Faster grid",
            "html_url": "https://github.com/hadinatajenta/cottondata/releases/tag/v0.11.0",
            "published_at": "2026-11-26T12:00:00Z",
            "assets": [
                {
                    "name": "CottonData-macos-arm64.zip",
                    "browser_download_url": "https://example.com/CottonData-macos-arm64.zip",
                    "size": 12345678
                },
                {
                    "name": "SHA256SUMS.txt",
                    "browser_download_url": "https://example.com/SHA256SUMS.txt",
                    "size": 512
                }
            ]
        }"###;
        let parsed = parse_github_release_json(payload).expect("valid release JSON");
        assert_eq!(parsed.tag_name, "v0.11.0");
        assert_eq!(parsed.assets.len(), 2);
        assert_eq!(parsed.assets[0].name, "CottonData-macos-arm64.zip");
        assert_eq!(parsed.assets[0].size, 12_345_678);
    }
}

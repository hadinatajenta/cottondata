use cottondata_core::{
    ConnectionEndpoint, EnvironmentTag, ServerProfile, TlsMode, DEFAULT_GRID_BUDGET_BYTES,
    MAX_SERVERS, MAX_SOCKETS_PER_SERVER,
};
use cottondata_grid::{export_csv, export_json_array, export_json_lines, export_sql_inserts};
use cottondata_introspect::SchemaCatalog;
use cottondata_mysql::{MySqlSession, QueryOutcome};
use cottondata_storage::AppStorage;
use mimalloc::MiMalloc;
use std::env;
use std::io::{self, Write};
use std::process;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help" | "help") {
        print_usage();
        return;
    }

    if let Err(err) = run(&args) {
        eprintln!("error: {err}");
        process::exit(1);
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let mut storage = AppStorage::open_default().map_err(|e| e.to_string())?;
    let mut registry = storage.load_registry().map_err(|e| e.to_string())?;

    match args[0].as_str() {
        "doctor" => {
            println!("CottonData CLI v{}", env!("CARGO_PKG_VERSION"));
            println!("Engine:       MySQL-only (pure Rust + rustls)");
            println!("Server Cap:   {} / {} slots configured", registry.count(), MAX_SERVERS);
            println!("Socket Cap:   max {} sockets per server", MAX_SOCKETS_PER_SERVER);
            println!("Grid Budget:  {} MiB LRU ceiling", DEFAULT_GRID_BUDGET_BYTES / (1024 * 1024));
            println!("Allocator:    mimalloc");
            println!("Data Dir:     {}", storage.data_dir().display());
            Ok(())
        }
        "servers" => {
            let sub = args.get(1).map(|s| s.as_str()).unwrap_or("list");
            match sub {
                "list" => {
                    if registry.count() == 0 {
                        println!("No MySQL servers configured (0/{MAX_SERVERS} slots).");
                        return Ok(());
                    }
                    println!("Configured MySQL servers ({}/{MAX_SERVERS}):", registry.count());
                    for slot in registry.iter() {
                        let ro = if slot.profile.read_only { " [RO]" } else { "" };
                        let ssh = if slot.profile.ssh.enabled { " [SSH]" } else { "" };
                        println!(
                            "  {}  {:<20} {:<24} user={:<12} env={}{}{}",
                            slot.slot_id,
                            slot.profile.name,
                            slot.profile.endpoint.display_address(),
                            slot.profile.username,
                            slot.profile.environment.label(),
                            ro,
                            ssh,
                        );
                    }
                    Ok(())
                }
                "add" => {
                    if args.len() < 5 {
                        return Err("Usage: cottondata-cli servers add <name> <host:port> <user> [schema] [--prod|--staging|--dev] [--ro]".into());
                    }
                    let name = &args[2];
                    let (host, port) = parse_host_port(&args[3])?;
                    let user = &args[4];
                    let mut profile = ServerProfile::new_local(name, host, port, user);
                    for extra in &args[5..] {
                        match extra.as_str() {
                            "--prod" => profile.environment = EnvironmentTag::Production,
                            "--staging" => profile.environment = EnvironmentTag::Staging,
                            "--dev" => profile.environment = EnvironmentTag::Development,
                            "--ro" => profile.read_only = true,
                            "--tls-disabled" => profile.tls = TlsMode::Disabled,
                            "--tls-required" => profile.tls = TlsMode::Required,
                            s if !s.starts_with('-') => profile.default_schema = Some(s.to_string()),
                            _ => {}
                        }
                    }
                    let slot_id = registry.add(profile).map_err(|e| e.to_string())?;
                    storage.save_registry(&registry).map_err(|e| e.to_string())?;
                    println!("Added MySQL server in slot {slot_id} ({}/{MAX_SERVERS} used).", registry.count());
                    Ok(())
                }
                "remove" => {
                    let target = args
                        .get(2)
                        .ok_or_else(|| "Usage: cottondata-cli servers remove <name-or-slot>".to_string())?;
                    let slot_id = find_slot(&registry, target)?;
                    let removed = registry.remove(slot_id).map_err(|e| e.to_string())?;
                    storage.delete_password(&removed.profile.id);
                    storage.save_registry(&registry).map_err(|e| e.to_string())?;
                    println!("Removed server '{}' from slot {slot_id}.", removed.profile.name);
                    Ok(())
                }
                other => Err(format!("Unknown servers subcommand '{other}'")),
            }
        }
        "test" => {
            let target = args.get(1).ok_or("Usage: cottondata-cli test <server>")?;
            let slot_id = find_slot(&registry, target)?;
            let profile = registry.get(slot_id).unwrap().profile.clone();
            let password = resolve_password(&mut storage, &profile.id);
            let session = MySqlSession::connect(&profile, password.as_deref())
                .map_err(|e| e.to_string())?;
            let rt = session.runtime();
            println!(
                "Connected to '{}' ({}) — MySQL {} | conn_id={} | tz={} | explain_analyze={}",
                profile.name,
                profile.endpoint.display_address(),
                rt.version,
                rt.connection_id,
                rt.time_zone,
                rt.supports_explain_analyze()
            );
            Ok(())
        }
        "schemas" => {
            let target = args.get(1).ok_or("Usage: cottondata-cli schemas <server>")?;
            let slot_id = find_slot(&registry, target)?;
            let profile = registry.get(slot_id).unwrap().profile.clone();
            let password = resolve_password(&mut storage, &profile.id);
            let mut session = MySqlSession::connect(&profile, password.as_deref())
                .map_err(|e| e.to_string())?;
            let mut catalog = SchemaCatalog::new();
            catalog.introspect_l0(&mut session).map_err(|e| e.to_string())?;
            for s in catalog.schemas() {
                let tag = if s.is_system { " [system]" } else { "" };
                println!("{}{}", catalog.resolve(s.name), tag);
            }
            Ok(())
        }
        "query" => {
            if args.len() < 3 {
                return Err("Usage: cottondata-cli query <server> <sql> [--limit N] [--format csv|jsonl|json|sql]".into());
            }
            let target = &args[1];
            let sql = &args[2];
            let mut limit = 500u32;
            let mut format = "csv".to_string();
            let mut i = 3;
            while i < args.len() {
                if args[i] == "--limit" && i + 1 < args.len() {
                    limit = args[i + 1].parse().unwrap_or(500);
                    i += 2;
                } else if args[i] == "--format" && i + 1 < args.len() {
                    format = args[i + 1].clone();
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let slot_id = find_slot(&registry, target)?;
            let profile = registry.get(slot_id).unwrap().profile.clone();
            let password = resolve_password(&mut storage, &profile.id);
            let mut session = MySqlSession::connect(&profile, password.as_deref())
                .map_err(|e| e.to_string())?;
            let outcome = session.execute_sql(sql, limit).map_err(|e| e.to_string())?;
            let mut stdout = io::stdout().lock();
            match outcome {
                QueryOutcome::Rows(page) => {
                    match format.as_str() {
                        "jsonl" => export_json_lines(&page, &mut stdout),
                        "json" => export_json_array(&page, &mut stdout),
                        "sql" => export_sql_inserts(&page, "result", &mut stdout),
                        _ => export_csv(&page, &mut stdout),
                    }
                    .map_err(|e| e.to_string())?;
                    stdout.flush().map_err(|e| e.to_string())?;
                }
                QueryOutcome::Affected {
                    affected_rows,
                    elapsed_ms,
                    ..
                } => {
                    writeln!(stdout, "OK ({affected_rows} rows affected, {elapsed_ms} ms)")
                        .map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        }
        other => Err(format!("Unknown command '{other}'. Run `cottondata-cli --help`.")),
    }
}

fn find_slot(
    registry: &cottondata_core::ServerRegistry,
    query: &str,
) -> Result<cottondata_core::ServerSlotId, String> {
    if let Ok(num) = query.trim_start_matches('#').parse::<usize>() {
        if (1..=MAX_SERVERS).contains(&num) {
            let id = cottondata_core::ServerSlotId::new(num - 1).unwrap();
            if registry.get(id).is_some() {
                return Ok(id);
            }
        }
    }
    registry
        .iter()
        .find(|s| s.profile.name.eq_ignore_ascii_case(query) || s.profile.id == query)
        .map(|s| s.slot_id)
        .ok_or_else(|| format!("Server '{query}' not found"))
}

fn parse_host_port(spec: &str) -> Result<(String, u16), String> {
    if let Some((h, p)) = spec.rsplit_once(':') {
        let port = p.parse::<u16>().map_err(|_| format!("Invalid port in '{spec}'"))?;
        Ok((h.to_string(), port))
    } else {
        Ok((spec.to_string(), 3306))
    }
}

fn resolve_password(storage: &mut AppStorage, profile_id: &str) -> Option<String> {
    env::var("COTTONDATA_PASSWORD")
        .ok()
        .or_else(|| storage.get_password(profile_id))
}

fn print_usage() {
    println!(
        "CottonData CLI — Featherweight MySQL-only client (max {MAX_SERVERS} servers)\n\n\
         USAGE:\n  \
           cottondata-cli doctor\n  \
           cottondata-cli servers list\n  \
           cottondata-cli servers add <name> <host:port> <user> [schema] [--prod|--staging|--dev] [--ro]\n  \
           cottondata-cli servers remove <name-or-slot>\n  \
           cottondata-cli test <server>\n  \
           cottondata-cli schemas <server>\n  \
           cottondata-cli query <server> <sql> [--limit 500] [--format csv|jsonl|json|sql]\n\n\
         Tip: Pass password via macOS Keychain (saved in GUI) or COTTONDATA_PASSWORD env var."
    );
}

#[allow(dead_code)]
fn _endpoint_check(e: &ConnectionEndpoint) -> String {
    e.display_address()
}

use cottondata_core::SshTunnelConfig;
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub struct SshTunnel {
    child: Child,
    local_port: u16,
}

impl SshTunnel {
    pub fn open(
        cfg: &SshTunnelConfig,
        remote_mysql_host: &str,
        remote_mysql_port: u16,
        timeout: Duration,
    ) -> Result<Self, String> {
        let local_port = match cfg.local_bind_port {
            Some(p) if p > 0 => p,
            _ => pick_free_local_port()?,
        };

        let mut cmd = Command::new("ssh");
        cmd.arg("-N")
            .arg("-o")
            .arg("BatchMode=yes")
            .arg("-o")
            .arg("ExitOnForwardFailure=yes")
            .arg("-o")
            .arg("ServerAliveInterval=30")
            .arg("-o")
            .arg("ServerAliveCountMax=3")
            .arg("-o")
            .arg(format!("ConnectTimeout={}", timeout.as_secs().max(3)))
            .arg("-p")
            .arg(cfg.port.max(1).to_string())
            .arg("-L")
            .arg(format!(
                "127.0.0.1:{local_port}:{remote_mysql_host}:{remote_mysql_port}"
            ));

        if let Some(key) = cfg.identity_file.as_deref().filter(|s| !s.trim().is_empty()) {
            cmd.arg("-i").arg(key);
        }

        cmd.arg(format!("{}@{}", cfg.username.trim(), cfg.host.trim()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to launch system ssh for tunnel: {e}"))?;

        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(Some(status)) = child.try_wait() {
                let mut err_buf = String::new();
                if let Some(mut stderr) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut stderr, &mut err_buf);
                }
                return Err(format!(
                    "SSH tunnel exited early ({status}): {}",
                    err_buf.trim()
                ));
            }
            if TcpStream::connect_timeout(
                &format!("127.0.0.1:{local_port}").parse().unwrap(),
                Duration::from_millis(120),
            )
            .is_ok()
            {
                return Ok(Self { child, local_port });
            }
            thread::sleep(Duration::from_millis(75));
        }

        let _ = child.kill();
        let _ = child.wait();
        Err(format!(
            "Timed out waiting for SSH tunnel 127.0.0.1:{local_port} -> {remote_mysql_host}:{remote_mysql_port}"
        ))
    }

    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for SshTunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pick_free_local_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Could not allocate ephemeral local port for SSH tunnel: {e}"))?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| format!("Could not inspect ephemeral local port: {e}"))
}

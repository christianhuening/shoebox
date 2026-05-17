//! mDNS service broadcaster. Announces _shoebox._tcp.local with TXT
//! records so LAN clients can auto-discover the server.

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use std::collections::HashMap;
use std::net::IpAddr;

pub const SERVICE_TYPE: &str = "_shoebox._tcp.local.";

pub struct MdnsBroadcaster {
    daemon: ServiceDaemon,
    fullname: String,
}

impl MdnsBroadcaster {
    /// Begin broadcasting. Returns immediately; the daemon broadcasts
    /// in the background until `shutdown()` is called.
    pub fn start(
        server_name: &str,
        port: u16,
        schema_version: i64,
        ips: Vec<IpAddr>,
    ) -> Result<Self> {
        let daemon = ServiceDaemon::new().context("creating mdns daemon")?;
        let host_label = sanitize(server_name);
        let fullname = format!("{host_label}.{SERVICE_TYPE}");

        let mut txt = HashMap::new();
        txt.insert("name".to_string(), server_name.to_string());
        txt.insert("schema".to_string(), schema_version.to_string());
        txt.insert("proto".to_string(), "libsql".to_string());

        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &host_label,
            &format!("{host_label}.local."),
            ips.as_slice(),
            port,
            Some(txt),
        )
        .context("building ServiceInfo")?;

        daemon.register(info).context("registering mdns service")?;
        tracing::info!(
            event = "mdns.register",
            service = SERVICE_TYPE,
            name = %server_name,
            port,
            "mDNS service registered"
        );

        Ok(Self { daemon, fullname })
    }

    pub fn shutdown(&self) {
        let _ = self.daemon.unregister(&self.fullname);
        let _ = self.daemon.shutdown();
        tracing::info!(event = "mdns.unregister", "mDNS service unregistered");
    }
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect()
}

/// Enumerate non-loopback IPs from local network interfaces.
pub fn local_ips() -> Vec<IpAddr> {
    // Use `if_addrs` for cross-platform interface enumeration.
    // For now keep it minimal: read from std until we add the dep.
    // mdns-sd accepts an empty list and will try to use all interfaces.
    Vec::new()
}

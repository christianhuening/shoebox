//! mDNS discovery of shoebox-server instances on the LAN, plus a
//! manual-entry path for cases where mDNS isn't available.

use anyhow::{Context, Result};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::sync::Arc;
use tokio::sync::mpsc;

const SERVICE_TYPE: &str = "_shoebox._tcp.local.";

#[derive(Debug, Clone)]
pub struct DiscoveredServer {
    /// Server-friendly name from the mDNS TXT record (or the user-supplied
    /// label for a manually-entered server).
    pub display_name: String,
    /// `https://host:port` URL the client connects to.
    pub url: String,
    /// True if this entry came from the user typing a URL rather than mDNS.
    pub manual: bool,
}

pub struct Browser {
    /// Receives discovery events. Owned by the caller (the Iced
    /// subscription drains it into Messages).
    pub rx: mpsc::UnboundedReceiver<DiscoveredServer>,
    pub(crate) tx: mpsc::UnboundedSender<DiscoveredServer>,
    daemon: Arc<ServiceDaemon>,
}

/// Spawn a background thread that drains `ServiceEvent`s from a fresh
/// browse and forwards `ServiceResolved` hits into `tx` as
/// `DiscoveredServer { manual: false, .. }`.
fn spawn_browse_drainer(
    daemon: &ServiceDaemon,
    tx: mpsc::UnboundedSender<DiscoveredServer>,
) -> Result<()> {
    let event_rx = daemon
        .browse(SERVICE_TYPE)
        .context("registering mDNS browse")?;
    std::thread::spawn(move || {
        while let Ok(event) = event_rx.recv() {
            if let ServiceEvent::ServiceResolved(info) = event {
                let display_name = info
                    .get_property_val_str("name")
                    .unwrap_or_else(|| info.get_fullname())
                    .to_string();
                let port = info.get_port();
                if let Some(host_ip) = info.get_addresses().iter().next() {
                    let url = format!("https://{host_ip}:{port}");
                    if tx
                        .send(DiscoveredServer {
                            display_name,
                            url,
                            manual: false,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });
    Ok(())
}

impl Browser {
    /// Start browsing for `_shoebox._tcp.local.` services on the local
    /// network. Discovered servers stream into `rx`.
    ///
    /// # Errors
    /// Returns an error if the mDNS daemon can't be started.
    pub fn start() -> Result<Self> {
        let daemon = ServiceDaemon::new().context("starting mDNS daemon")?;
        let (tx, rx) = mpsc::unbounded_channel();
        spawn_browse_drainer(&daemon, tx.clone())?;
        Ok(Self {
            rx,
            tx,
            daemon: Arc::new(daemon),
        })
    }

    /// Inject a manually-entered server URL as if it were discovered.
    /// `display_name` is whatever the user typed (or a default).
    pub fn add_manual(&self, display_name: &str, url: &str) {
        let _ = self.tx.send(DiscoveredServer {
            display_name: display_name.to_string(),
            url: url.to_string(),
            manual: true,
        });
    }

    /// Re-arm the browse (used by the discovery screen's Retry button).
    ///
    /// # Errors
    /// Returns an error if the daemon's re-browse fails.
    pub fn rebrowse(&self) -> Result<()> {
        spawn_browse_drainer(&self.daemon, self.tx.clone())
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::{Browser, DiscoveredServer};

    #[tokio::test]
    async fn add_manual_emits_event() {
        let Ok(mut browser) = Browser::start() else {
            eprintln!("skipping: mDNS daemon not available");
            return;
        };
        // Push a synthetic event via the same private channel
        // `add_manual` would use, then drain it through the public `rx`.
        let _ = browser.tx.send(DiscoveredServer {
            display_name: "Manual".to_string(),
            url: "https://x:9000".to_string(),
            manual: true,
        });
        let received =
            tokio::time::timeout(std::time::Duration::from_millis(500), browser.rx.recv())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(received.url, "https://x:9000");
        assert!(received.manual);
    }
}

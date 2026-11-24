// SPDX-License-Identifier: LGPL-2.1-or-later
//! Finding the clients: every client sends a short announcement (UDP broadcast) every two
//! seconds; the server listens and keeps a list of the clients it heard recently.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::proto::Specs;

/// What a client announces.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Announce {
    /// Always "rmb-net" (other programs on the port are ignored).
    pub rmb: String,
    pub version: String,
    /// Random per client start.
    pub id: u64,
    pub name: String,
    /// TCP port the client accepts the server on.
    pub port: u16,
    /// Rendering for a server right now.
    pub busy: bool,
    /// The client needs the shared key.
    pub key: bool,
    pub specs: Specs,
}

const MAGIC: &str = "rmb-net";
/// Time between announcements.
const EVERY: Duration = Duration::from_secs(2);
/// A client not heard for this long is gone.
const GONE: Duration = Duration::from_secs(7);

impl Announce {
    pub fn new(id: u64, name: String, port: u16, key: bool, specs: Specs) -> Announce {
        Announce { rmb: MAGIC.into(), version: crate::version(), id, name, port, busy: false, key, specs }
    }
}

/// Sends `info` every two seconds (broadcast and to this machine) until `stop` is set.
/// Returns at once; the announcements run on a thread.
pub fn announce(info: Arc<Mutex<Announce>>, stop: Arc<AtomicBool>) -> std::io::Result<()> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    sock.set_broadcast(true)?;
    std::thread::spawn(move || {
        let mut targets = Vec::new();
        let mut round = 0u32;
        while !stop.load(Ordering::Relaxed) {
            // The interfaces can change (Wi-Fi, VPN): look again every half minute.
            if round.is_multiple_of(15) {
                targets = broadcast_targets();
            }
            round = round.wrapping_add(1);
            let msg = info.lock().map(|i| serde_json::to_vec(&*i).unwrap_or_default()).unwrap_or_default();
            for t in &targets {
                // A network without broadcast still reaches a server on this machine.
                let _ = sock.send_to(&msg, t);
            }
            std::thread::sleep(EVERY);
        }
    });
    Ok(())
}

/// Where announcements go: the broadcast address of every IPv4 network of this machine
/// (macOS and Windows do not pass 255.255.255.255 on to every interface), the general
/// broadcast address, and this machine.
fn broadcast_targets() -> Vec<SocketAddr> {
    let mut out = vec![SocketAddr::from((Ipv4Addr::BROADCAST, crate::DISCOVERY_PORT)), SocketAddr::from((Ipv4Addr::LOCALHOST, crate::DISCOVERY_PORT))];
    let Ok(nets) = sysinfo::Networks::new_with_refreshed_list() else { return out };
    for data in nets.values() {
        for n in data.ip_networks() {
            if let IpAddr::V4(a) = n.addr
                && !a.is_loopback()
                && !a.is_link_local()
                && (1..=30).contains(&n.prefix)
            {
                let b = SocketAddr::from((subnet_broadcast(a, n.prefix), crate::DISCOVERY_PORT));
                if !out.contains(&b) {
                    out.push(b);
                }
            }
        }
    }
    out
}

/// The broadcast address of the network `addr/prefix`.
fn subnet_broadcast(addr: Ipv4Addr, prefix: u8) -> Ipv4Addr {
    let mask = u32::MAX.checked_shl(32 - prefix as u32).unwrap_or(0);
    Ipv4Addr::from(u32::from(addr) | !mask)
}

/// A client the server heard.
#[derive(Clone, Debug)]
pub struct Seen {
    pub info: Announce,
    /// Where to connect (sender address, announced port).
    pub addr: SocketAddr,
    pub last: Instant,
}

/// The server's list of clients, filled by a listening thread.
pub struct Discovery {
    seen: Arc<Mutex<HashMap<u64, Seen>>>,
    stop: Arc<AtomicBool>,
}

impl Discovery {
    /// Starts listening on the discovery port.
    pub fn start() -> Result<Discovery, String> {
        let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, crate::DISCOVERY_PORT)).map_err(|e| format!("cannot listen for clients on UDP port {} ({e}); is another server running on this machine?", crate::DISCOVERY_PORT))?;
        sock.set_read_timeout(Some(Duration::from_millis(300))).map_err(|e| e.to_string())?;
        let seen = Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (s2, stop2) = (seen.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 8192];
            while !stop2.load(Ordering::Relaxed) {
                let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
                let Ok(info) = serde_json::from_slice::<Announce>(&buf[..n]) else { continue };
                if info.rmb != MAGIC {
                    continue;
                }
                let addr = SocketAddr::new(from.ip(), info.port);
                if let Ok(mut m) = s2.lock() {
                    // The same client over loopback and the network: keep the network address.
                    let keep_old = m.get(&info.id).is_some_and(|old: &Seen| from.ip().is_loopback() && !old.addr.ip().is_loopback() && old.last.elapsed() < GONE);
                    let addr = if keep_old { m[&info.id].addr } else { addr };
                    m.insert(info.id, Seen { info, addr, last: Instant::now() });
                }
            }
        });
        Ok(Discovery { seen, stop })
    }

    /// The clients heard in the last seconds, sorted by name.
    pub fn clients(&self) -> Vec<Seen> {
        let mut v: Vec<Seen> = self.seen.lock().map(|m| m.values().filter(|s| s.last.elapsed() < GONE).cloned().collect()).unwrap_or_default();
        v.sort_by(|a, b| a.info.name.cmp(&b.info.name).then(a.info.id.cmp(&b.info.id)));
        v
    }

    /// Waits up to `time` and returns the clients heard by then.
    pub fn wait(&self, time: Duration) -> Vec<Seen> {
        std::thread::sleep(time);
        self.clients()
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A client given by address (networks that block broadcasts): `host` or `host:port`.
pub fn parse_address(s: &str) -> Result<SocketAddr, String> {
    use std::net::ToSocketAddrs;
    let with_port = if s.parse::<IpAddr>().is_ok() || !s.contains(':') { format!("{s}:{}", crate::CLIENT_PORT) } else { s.to_string() };
    with_port.to_socket_addrs().map_err(|e| format!("{s}: {e}"))?.next().ok_or_else(|| format!("{s}: no address"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnet_broadcast_addresses() {
        assert_eq!(subnet_broadcast(Ipv4Addr::new(192, 168, 1, 23), 24), Ipv4Addr::new(192, 168, 1, 255));
        assert_eq!(subnet_broadcast(Ipv4Addr::new(10, 1, 2, 3), 8), Ipv4Addr::new(10, 255, 255, 255));
        assert!(broadcast_targets().len() >= 2);
    }
}

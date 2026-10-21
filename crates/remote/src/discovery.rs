//! Finding other Unsub libraries on the local network with mDNS (Bonjour), the way
//! iTunes 10 found shared libraries.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::Serialize;

use crate::error::Result;

const SERVICE_TYPE: &str = "_unsub._tcp.local.";
/// How long dropping an `Advertisement` waits for its goodbye packets to go out.
const GOODBYE_WAIT: Duration = Duration::from_secs(1);

/// Another library on the network: an Unsub with sharing on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    /// What its owner called it, such as "Riley's MacBook".
    pub name: String,
    /// Its network name, such as `rileys-macbook.local`.
    pub host: String,
    pub port: u16,
    /// IPv4 addresses first. An IPv6 link-local address needs its interface to be used, which
    /// is not recorded here, so prefer the first one.
    pub addresses: Vec<IpAddr>,
}

/// The process's one mDNS daemon, started when first asked for. Each daemon is a thread and a set of
/// sockets on port 5353, so browsing for libraries, announcing this one and browsing for speakers
/// all use this one rather than a daemon each.
pub fn shared_daemon() -> Result<ServiceDaemon> {
    static DAEMON: Mutex<Option<ServiceDaemon>> = Mutex::new(None);
    let mut slot = DAEMON.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(daemon) = slot.as_ref() {
        return Ok(daemon.clone());
    }
    let daemon = ServiceDaemon::new()?;
    *slot = Some(daemon.clone());
    Ok(daemon)
}

/// Keeps this library announced on the network; dropping it withdraws the announcement.
pub struct Advertisement {
    daemon: ServiceDaemon,
    fullname: String,
}

/// Announces a Subsonic server on `port` as `_unsub._tcp`, with TXT records
/// `version=1` and `api=subsonic`, under the display name `name`.
pub fn advertise(name: &str, port: u16) -> Result<Advertisement> {
    let info = service_info(name, port)?;
    let advertisement = Advertisement {
        daemon: shared_daemon()?,
        fullname: info.get_fullname().to_owned(),
    };
    advertisement.daemon.register(info)?;
    Ok(advertisement)
}

impl Drop for Advertisement {
    fn drop(&mut self) {
        // Peers drop a library from their list when told it is leaving, instead of waiting for
        // its records to expire.
        if let Ok(done) = self.daemon.unregister(&self.fullname) {
            let _ = done.recv_timeout(GOODBYE_WAIT);
        }
    }
}

/// Watches for other libraries. Dropping it stops the search.
pub struct Browser {
    daemon: ServiceDaemon,
}

/// Calls `on_change` with the complete list of peers each time one appears or disappears.
/// The callback runs on a background thread. This machine's own library shows up in the list too
/// if it is advertising; the app can tell it by name.
pub fn browse(on_change: impl Fn(Vec<Peer>) + Send + 'static) -> Result<Browser> {
    let daemon = shared_daemon()?;
    let events = daemon.browse(SERVICE_TYPE)?;
    thread::spawn(move || {
        let mut peers: BTreeMap<String, Peer> = BTreeMap::new();
        // Ends when the daemon shuts down and the channel closes.
        while let Ok(event) = events.recv() {
            let changed = match event {
                ServiceEvent::ServiceResolved(service) if service.is_valid() => {
                    let peer = Peer {
                        name: instance_name(&service.fullname, &service.ty_domain),
                        host: service.host.trim_end_matches('.').to_owned(),
                        port: service.port,
                        addresses: sorted(service.addresses.iter().map(|ip| ip.to_ip_addr())),
                    };
                    peers.insert(service.fullname.clone(), peer.clone()) != Some(peer)
                }
                ServiceEvent::ServiceRemoved(_, fullname) => peers.remove(&fullname).is_some(),
                _ => false,
            };
            if changed {
                on_change(peers.values().cloned().collect());
            }
        }
    });
    Ok(Browser { daemon })
}

impl Drop for Browser {
    fn drop(&mut self) {
        // Closes the channel the thread reads, which ends it. The daemon is shared, and stays.
        let _ = self.daemon.stop_browse(SERVICE_TYPE);
    }
}

/// The record that announces the library. The host name is made from the display name because
/// mDNS host names cannot hold spaces or apostrophes.
fn service_info(name: &str, port: u16) -> Result<ServiceInfo> {
    let host = format!("{}.local.", host_label(name));
    let properties = [("version", "1"), ("api", "subsonic")];
    // An empty address list plus `enable_addr_auto` has the daemon announce every address of
    // this machine, and keep that right as networks come and go.
    Ok(ServiceInfo::new(SERVICE_TYPE, name, &host, "", port, &properties[..])?.enable_addr_auto())
}

fn host_label(name: &str) -> String {
    let dashed: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let label = dashed
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if label.is_empty() {
        return "unsub".to_owned();
    }
    // A DNS label is at most 63 bytes.
    label.chars().take(63).collect()
}

/// "Mr\. Smith's Mac._unsub._tcp.local." is the instance "Mr. Smith's Mac".
pub fn instance_name(fullname: &str, service_type: &str) -> String {
    let escaped = fullname
        .strip_suffix(service_type)
        .map_or(fullname, |name| name.strip_suffix('.').unwrap_or(name));
    escaped.replace("\\.", ".").replace("\\\\", "\\")
}

/// IPv4 addresses first, each in order.
pub fn sorted(addresses: impl Iterator<Item = IpAddr>) -> Vec<IpAddr> {
    let mut addresses: Vec<IpAddr> = addresses.collect();
    addresses.sort_by_key(|address| (address.is_ipv6(), *address));
    addresses
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_record_carries_the_documented_txt_properties() {
        let info = service_info("Riley's MacBook", 4533).unwrap();

        assert_eq!(info.get_property_val_str("version"), Some("1"));
        assert_eq!(info.get_property_val_str("api"), Some("subsonic"));
        assert_eq!(info.get_port(), 4533);
        assert_eq!(info.get_type(), "_unsub._tcp.local.");
        assert_eq!(info.get_fullname(), "Riley's MacBook._unsub._tcp.local.");
        assert_eq!(info.get_hostname(), "riley-s-macbook.local.");
        assert!(info.is_addr_auto());
    }

    #[test]
    fn host_labels_are_valid_dns_labels() {
        assert_eq!(host_label("Riley's MacBook Pro"), "riley-s-macbook-pro");
        assert_eq!(host_label("  --  "), "unsub");
        assert_eq!(host_label("Zoë"), "zo");
        assert_eq!(host_label(&"a".repeat(100)).len(), 63);
    }

    #[test]
    fn instance_names_lose_the_service_suffix_and_escapes() {
        let ty = "_unsub._tcp.local.";
        assert_eq!(instance_name("Den._unsub._tcp.local.", ty), "Den");
        assert_eq!(
            instance_name("Mr\\. Smith's Mac._unsub._tcp.local.", ty),
            "Mr. Smith's Mac"
        );
        assert_eq!(instance_name("odd", ty), "odd");
    }

    #[test]
    fn addresses_list_ipv4_first() {
        let v6: IpAddr = "fe80::1".parse().unwrap();
        let low: IpAddr = "10.0.0.2".parse().unwrap();
        let high: IpAddr = "192.168.1.5".parse().unwrap();
        assert_eq!(sorted([v6, high, low].into_iter()), [low, high, v6]);
    }

    /// Needs working multicast on a real network interface, which sandboxes and some CI machines
    /// lack, so it is run on purpose: `cargo test -p unsub-remote -- --ignored`.
    #[test]
    #[ignore = "needs multicast networking"]
    fn a_browser_sees_an_advertisement_and_its_withdrawal() {
        use std::sync::mpsc::channel;

        let (seen, lists) = channel();
        let _browser = browse(move |peers| {
            let _ = seen.send(peers);
        })
        .unwrap();
        let name = format!("Unsub loopback test {}", std::process::id());
        let advertisement = advertise(&name, 4533).unwrap();

        let found = loop {
            let peers = lists
                .recv_timeout(Duration::from_secs(20))
                .expect("a peer to appear");
            if let Some(peer) = peers.into_iter().find(|peer| peer.name == name) {
                break peer;
            }
        };
        assert_eq!(found.port, 4533);
        assert!(!found.addresses.is_empty());

        drop(advertisement);
        loop {
            let peers = lists
                .recv_timeout(Duration::from_secs(20))
                .expect("the peer to disappear");
            if peers.iter().all(|peer| peer.name != name) {
                break;
            }
        }
    }
}

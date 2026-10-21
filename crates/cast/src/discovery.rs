//! Finding speakers on the local network: Chromecasts announce themselves over mDNS
//! (`_googlecast._tcp`), and UPnP renderers, Sonos among them, answer an SSDP search.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use mdns_sd::{ServiceDaemon, ServiceEvent};
use unsub_remote::{instance_name, shared_daemon, sorted as ipv4_first};

use crate::device::{Device, Endpoint, Kind};
use crate::upnp::{self, Role, SSDP_GROUP, parse_description, parse_search_reply, search_request};
use crate::{Result, chromecast};

const CHROMECAST_SERVICE: &str = "_googlecast._tcp.local.";

/// How long after the last sign of life a UPnP speaker is still listed. It is searched for far more
/// often than this, so a speaker that drops out has been missed several times over.
const UPNP_LIFETIME: Duration = Duration::from_secs(100);
/// When searching, relative to the start of the search: speakers answer at random within a couple
/// of seconds, and a few repeats catch the ones that miss a packet.
const SEARCH_ROUNDS: [Duration; 3] = [
    Duration::ZERO,
    Duration::from_secs(3),
    Duration::from_secs(8),
];
/// Between searches once the network has been looked over.
const SEARCH_EVERY: Duration = Duration::from_secs(30);
const LISTEN_SLICE: Duration = Duration::from_millis(400);
const DESCRIPTION_TIMEOUT: Duration = Duration::from_secs(3);
/// A device description is a few kilobytes; one this large is not one.
const DESCRIPTION_LIMIT: u64 = 256 * 1024;

/// A speaker an mDNS announcement describes. `name` is the TXT record `fn`, the name its owner
/// chose; without one the service name is all there is to go by.
pub(crate) fn chromecast_from(
    uuid: Option<&str>,
    friendly_name: Option<&str>,
    fallback_name: &str,
    port: u16,
    addresses: impl Iterator<Item = IpAddr>,
) -> Option<Device> {
    // An IPv6 link-local address needs its interface to be reached, which is not known here.
    let address = *ipv4_first(addresses).first()?;
    let uuid = uuid.filter(|uuid| !uuid.is_empty())?;
    Some(Device {
        id: format!("cast:{uuid}"),
        name: friendly_name
            .filter(|name| !name.is_empty())
            .unwrap_or(fallback_name)
            .to_owned(),
        kind: Kind::Chromecast,
        address,
        endpoint: Endpoint::Chromecast {
            port: if port == 0 { chromecast::PORT } else { port },
        },
    })
}

struct Entry {
    device: Device,
    /// `None` for speakers announced over mDNS, which tell us when they leave.
    expires: Option<Instant>,
}

/// Every speaker found so far, and the callback that hears when the list changes.
struct Registry {
    entries: Mutex<HashMap<String, Entry>>,
    on_change: Box<dyn Fn(Vec<Device>) + Send + Sync>,
}

impl Registry {
    fn entries(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn announce(&self, entries: &HashMap<String, Entry>) {
        (self.on_change)(ordered(entries.values().map(|entry| entry.device.clone())));
    }

    fn add(&self, device: Device, expires: Option<Instant>) {
        let mut entries = self.entries();
        let changed = entries
            .get(&device.id)
            .is_none_or(|entry| entry.device != device);
        entries.insert(device.id.clone(), Entry { device, expires });
        if changed {
            self.announce(&entries);
        }
    }

    fn remove(&self, id: &str) {
        let mut entries = self.entries();
        if entries.remove(id).is_some() {
            self.announce(&entries);
        }
    }

    fn expire(&self, now: Instant) {
        let mut entries = self.entries();
        let before = entries.len();
        entries.retain(|_, entry| entry.expires.is_none_or(|expires| expires > now));
        if entries.len() != before {
            self.announce(&entries);
        }
    }
}

/// Chromecasts, then Sonos, then the rest, each by name, which is how the menu groups them.
fn ordered(devices: impl Iterator<Item = Device>) -> Vec<Device> {
    let mut devices: Vec<Device> = devices.collect();
    let rank = |kind: Kind| match kind {
        Kind::Chromecast => 0,
        Kind::Sonos => 1,
        Kind::Upnp => 2,
    };
    devices.sort_by_cached_key(|device| {
        (
            rank(device.kind),
            device.name.to_lowercase(),
            device.id.clone(),
        )
    });
    devices
}

/// Watches for speakers. Dropping it stops the search.
pub struct Browser {
    daemon: ServiceDaemon,
    stop: Arc<AtomicBool>,
    search: Sender<()>,
}

/// Starts looking for speakers. `on_change` is called, on a background thread, with the complete
/// list each time a speaker appears, changes or disappears.
pub fn browse(on_change: impl Fn(Vec<Device>) + Send + Sync + 'static) -> Result<Browser> {
    let registry = Arc::new(Registry {
        entries: Mutex::default(),
        on_change: Box::new(on_change),
    });
    let stop = Arc::new(AtomicBool::new(false));

    let daemon = shared_daemon()?;
    let events = daemon.browse(CHROMECAST_SERVICE)?;
    {
        let registry = Arc::clone(&registry);
        thread::Builder::new()
            .name("unsub-cast-mdns".into())
            .spawn(move || {
                // A removal names the service, not the id the speaker was listed under.
                let mut listed: HashMap<String, String> = HashMap::new();
                // Ends when the browse is stopped and the channel closes.
                while let Ok(event) = events.recv() {
                    match event {
                        ServiceEvent::ServiceResolved(service) => {
                            let name = instance_name(service.get_fullname(), CHROMECAST_SERVICE);
                            let addresses = service
                                .get_addresses()
                                .iter()
                                .map(|address| address.to_ip_addr());
                            let found = chromecast_from(
                                service.get_property_val_str("id"),
                                service.get_property_val_str("fn"),
                                &name,
                                service.get_port(),
                                addresses,
                            );
                            if let Some(device) = found {
                                listed.insert(service.get_fullname().to_owned(), device.id.clone());
                                registry.add(device, None);
                            }
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            if let Some(id) = listed.remove(&fullname) {
                                registry.remove(&id);
                            }
                        }
                        _ => {}
                    }
                }
            })?;
    }

    let (search, requests) = mpsc::channel();
    {
        let stop = Arc::clone(&stop);
        thread::Builder::new()
            .name("unsub-cast-ssdp".into())
            .spawn(move || search_loop(&registry, &stop, &requests))?;
    }
    Ok(Browser {
        daemon,
        stop,
        search,
    })
}

impl Browser {
    /// Looks again now, rather than at the next scheduled search. Used when the menu is opened.
    pub fn search_now(&self) {
        let _ = self.search.send(());
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // The daemon is the app's, and stays.
        let _ = self.daemon.stop_browse(CHROMECAST_SERVICE);
    }
}

/// Searches for UPnP renderers now and then, and drops those that stop answering.
fn search_loop(registry: &Registry, stop: &AtomicBool, requests: &mpsc::Receiver<()>) {
    let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else {
        return;
    };
    let _ = socket.set_multicast_ttl_v4(2);
    // Waiting for replies is what paces the loop.
    let _ = socket.set_read_timeout(Some(LISTEN_SLICE));
    let mut round_start = Instant::now();
    let mut next_round = 0;
    let mut next_search = round_start;
    while !stop.load(Ordering::Relaxed) {
        match requests.try_recv() {
            // A request to search now restarts the schedule; several in a row are one request.
            Ok(()) => {
                while requests.try_recv().is_ok() {}
                next_search = Instant::now();
            }
            Err(TryRecvError::Disconnected) => return,
            Err(TryRecvError::Empty) => {}
        }
        let now = Instant::now();
        if now >= next_search {
            (round_start, next_round, next_search) = (now, 0, now + SEARCH_EVERY);
        }
        if let Some(offset) = SEARCH_ROUNDS.get(next_round)
            && now >= round_start + *offset
        {
            // Without a network the send fails; the next search tries again.
            let _ = socket.send_to(search_request().as_bytes(), SSDP_GROUP);
            next_round += 1;
        }
        let mut datagram = [0_u8; 2048];
        match socket.recv_from(&mut datagram) {
            Ok((read, from)) => {
                if let Ok(text) = std::str::from_utf8(&datagram[..read])
                    && let Some(reply) = parse_search_reply(text)
                {
                    match describe(&reply.location, from) {
                        Some(Found::Speaker(device)) => {
                            registry.add(device, Some(Instant::now() + UPNP_LIFETIME))
                        }
                        // It may have been a room of its own until it was grouped.
                        Some(Found::Follower(id)) => registry.remove(&id),
                        None => {}
                    }
                }
            }
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            // A socket that fails at once, as Windows' does after a send to an unreachable group, must not spin.
            Err(_) => thread::sleep(LISTEN_SLICE),
        }
        registry.expire(Instant::now());
    }
}

/// What a speaker's description makes of it.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    Speaker(Device),
    /// The speaker with this id plays only what another does, as half of a stereo pair, a sub or a
    /// room in someone else's group, and is not listed.
    Follower(String),
}

/// Fetches and reads the description at `location`, which must be on the machine that answered:
/// a speaker has no business pointing this app at anywhere else. A Sonos speaker is then asked how
/// the speakers are arranged, since bonded ones all answer the search but only one takes commands.
fn describe(location: &str, from: SocketAddr) -> Option<Found> {
    let host = location.strip_prefix("http://")?.split(['/', ':']).next()?;
    if host.parse::<IpAddr>().ok()? != from.ip() {
        return None;
    }
    let mut response = upnp::agent(DESCRIPTION_TIMEOUT).get(location).call().ok()?;
    if response.status() != 200 {
        return None;
    }
    let xml = response
        .body_mut()
        .with_config()
        .limit(DESCRIPTION_LIMIT)
        .read_to_string()
        .ok()?;
    let mut device = parse_description(&xml, location, from.ip())?;
    // A speaker that won't say how it is grouped is listed as it is.
    match upnp::zone_role(&device) {
        Ok(Some(Role::Follower)) => return Some(Found::Follower(device.id)),
        Ok(Some(Role::Primary(name))) if !name.is_empty() => device.name = name,
        _ => {}
    }
    Some(Found::Speaker(device))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::channel;

    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn a_chromecast_is_named_by_its_txt_record_and_reached_over_ipv4_first() {
        let device = chromecast_from(
            Some("9f1c2a"),
            Some("Kitchen display"),
            "Chromecast-9f1c2a",
            8009,
            [ip("fe80::1"), ip("192.168.1.40")].into_iter(),
        )
        .unwrap();
        assert_eq!(device.id, "cast:9f1c2a");
        assert_eq!(device.name, "Kitchen display");
        assert_eq!(device.kind, Kind::Chromecast);
        assert_eq!(device.address, ip("192.168.1.40"));
        assert_eq!(device.endpoint, Endpoint::Chromecast { port: 8009 });
    }

    #[test]
    fn a_chromecast_without_a_friendly_name_is_called_by_its_service_name() {
        let device = chromecast_from(
            Some("abc"),
            None,
            "Chromecast-abc",
            0,
            [ip("192.168.1.40")].into_iter(),
        )
        .unwrap();
        assert_eq!(device.name, "Chromecast-abc");
        assert_eq!(device.endpoint, Endpoint::Chromecast { port: 8009 });
        assert_eq!(
            chromecast_from(
                Some("abc"),
                Some(""),
                "Fallback",
                8009,
                [ip("192.168.1.40")].into_iter()
            )
            .unwrap()
            .name,
            "Fallback"
        );
    }

    #[test]
    fn an_announcement_without_an_address_or_an_id_is_not_a_speaker() {
        assert_eq!(
            chromecast_from(Some("abc"), Some("X"), "X", 8009, std::iter::empty()),
            None
        );
        assert_eq!(
            chromecast_from(None, Some("X"), "X", 8009, [ip("192.168.1.40")].into_iter()),
            None
        );
    }

    fn upnp(name: &str, kind: Kind) -> Device {
        Device {
            id: format!("upnp:{name}"),
            name: name.to_owned(),
            kind,
            address: ip("192.168.1.9"),
            endpoint: Endpoint::Chromecast { port: 1 },
        }
    }

    #[test]
    fn the_list_is_chromecasts_then_sonos_then_the_rest_each_by_name() {
        let list = ordered(
            [
                upnp("zeta", Kind::Upnp),
                upnp("Bath", Kind::Sonos),
                upnp("amp", Kind::Upnp),
                upnp("Den", Kind::Chromecast),
                upnp("Attic", Kind::Sonos),
            ]
            .into_iter(),
        );
        let names: Vec<&str> = list.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Den", "Attic", "Bath", "amp", "zeta"]);
    }

    #[test]
    fn the_callback_hears_only_real_changes() {
        let (heard, lists) = channel();
        let registry = Registry {
            entries: Mutex::default(),
            on_change: Box::new(move |list| heard.send(list).unwrap()),
        };
        let kitchen = upnp("Kitchen", Kind::Sonos);

        registry.add(kitchen.clone(), None);
        registry.add(kitchen.clone(), None);
        assert_eq!(lists.try_recv().unwrap().len(), 1);
        assert!(
            lists.try_recv().is_err(),
            "the same speaker found again is not news"
        );

        registry.add(
            Device {
                name: "Kitchen 2".into(),
                ..kitchen.clone()
            },
            None,
        );
        assert_eq!(lists.try_recv().unwrap()[0].name, "Kitchen 2");

        registry.remove(&kitchen.id);
        assert!(lists.try_recv().unwrap().is_empty());
        registry.remove(&kitchen.id);
        assert!(lists.try_recv().is_err());
    }

    #[test]
    fn a_speaker_that_stops_answering_leaves_the_list_but_an_announced_one_stays_until_it_leaves() {
        let (heard, lists) = channel();
        let registry = Registry {
            entries: Mutex::default(),
            on_change: Box::new(move |list| heard.send(list).unwrap()),
        };
        let now = Instant::now();
        registry.add(upnp("Sonos", Kind::Sonos), Some(now + UPNP_LIFETIME));
        registry.add(upnp("Cast", Kind::Chromecast), None);
        while lists.try_recv().is_ok() {}

        registry.expire(now + UPNP_LIFETIME - Duration::from_secs(1));
        assert!(lists.try_recv().is_err());
        registry.expire(now + UPNP_LIFETIME + Duration::from_secs(1));
        let names: Vec<String> = lists
            .try_recv()
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(names, ["Cast"]);
    }

    /// A speaker on a local socket. `handler` gets each request's head and body and answers with a status, a
    /// `Location` to send it to (if any) and a body.
    fn serve(
        handler: impl Fn(&str, &str) -> (u16, Option<String>, String) + Send + 'static,
    ) -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else {
                    return;
                };
                let mut buffered = Vec::new();
                let mut chunk = [0_u8; 4096];
                let (head, body) = loop {
                    let read = connection.read(&mut chunk).unwrap_or(0);
                    if read == 0 {
                        break (String::new(), String::new());
                    }
                    buffered.extend_from_slice(&chunk[..read]);
                    let text = String::from_utf8_lossy(&buffered).into_owned();
                    let Some((head, body)) = text.split_once("\r\n\r\n") else {
                        continue;
                    };
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|n| n.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if body.len() >= length {
                        break (head.to_owned(), body.to_owned());
                    }
                };
                let (status, location, body) = handler(&head, &body);
                let location = location
                    .map(|to| format!("Location: {to}\r\n"))
                    .unwrap_or_default();
                let _ = write!(
                    connection,
                    "HTTP/1.1 {status} X\r\nContent-Type: text/xml\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        address
    }

    /// Serves `body` as the description, whatever is asked.
    fn serve_description(body: &'static str) -> SocketAddr {
        serve(move |_, _| (200, None, body.to_owned()))
    }

    const DESCRIPTION: &str = r#"<root xmlns="urn:schemas-upnp-org:device-1-0"><device><friendlyName>Amp</friendlyName><UDN>uuid:amp-1</UDN>
        <serviceList><service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>/avt</controlURL></service></serviceList></device></root>"#;

    fn speaker(found: Option<Found>) -> Device {
        match found {
            Some(Found::Speaker(device)) => device,
            other => panic!("not listed as a speaker: {other:?}"),
        }
    }

    #[test]
    fn a_description_is_fetched_from_the_machine_that_answered() {
        let address = serve_description(DESCRIPTION);
        let device = speaker(describe(&format!("http://{address}/desc.xml"), address));
        assert_eq!(
            (device.id.as_str(), device.name.as_str(), device.kind),
            ("upnp:amp-1", "Amp", Kind::Upnp)
        );
    }

    #[test]
    fn a_description_somewhere_else_is_not_fetched() {
        let address = serve_description(DESCRIPTION);
        let elsewhere: SocketAddr = "192.168.1.77:1900".parse().unwrap();
        assert_eq!(
            describe(&format!("http://{address}/desc.xml"), elsewhere),
            None
        );
        assert_eq!(describe("http://localhost/desc.xml", address), None);
        assert_eq!(describe("https://127.0.0.1/desc.xml", address), None);
    }

    #[test]
    fn a_redirect_is_not_followed() {
        let elsewhere_asked = Arc::new(AtomicBool::new(false));
        let asked = Arc::clone(&elsewhere_asked);
        let elsewhere = serve(move |_, _| {
            asked.store(true, Ordering::Relaxed);
            (200, None, DESCRIPTION.to_owned())
        });
        let redirecting = serve(move |_, _| {
            (
                302,
                Some(format!("http://{elsewhere}/desc.xml")),
                String::new(),
            )
        });
        assert_eq!(
            describe(&format!("http://{redirecting}/desc.xml"), redirecting),
            None
        );
        assert!(!elsewhere_asked.load(Ordering::Relaxed));
    }

    /// A Sonos speaker's description, with the topology service, for the speaker `uuid`.
    fn sonos(uuid: &str) -> String {
        format!(
            r#"<root xmlns="urn:schemas-upnp-org:device-1-0"><device><manufacturer>Sonos, Inc.</manufacturer><friendlyName>192.168.1.23 - Sonos One - {uuid}</friendlyName>
            <roomName>Own room</roomName><UDN>uuid:{uuid}</UDN>
            <serviceList><service><serviceType>urn:schemas-upnp-org:service:ZoneGroupTopology:1</serviceType><controlURL>/zgt</controlURL></service></serviceList>
            <deviceList><device><serviceList><service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>/avt</controlURL></service></serviceList></device></deviceList>
            </device></root>"#
        )
    }

    /// The speakers' arrangement as a Sonos speaker reports it: the state is XML inside XML.
    fn topology_reply() -> String {
        let state = r#"<ZoneGroups>
            <ZoneGroup Coordinator="RINCON_KITCHEN01400"><ZoneGroupMember UUID="RINCON_KITCHEN01400" ZoneName="Kitchen"/><ZoneGroupMember UUID="RINCON_BEDROOM01400" ZoneName="Bedroom"/></ZoneGroup>
            <ZoneGroup Coordinator="RINCON_DEN01400"><ZoneGroupMember UUID="RINCON_DEN01400" ZoneName="Den"/><ZoneGroupMember UUID="RINCON_DENR01400" ZoneName="Den" Invisible="1"/></ZoneGroup>
            <ZoneGroup Coordinator="RINCON_ARC01400"><ZoneGroupMember UUID="RINCON_ARC01400" ZoneName="Living Room"><Satellite UUID="RINCON_SUB01400" Invisible="1"/></ZoneGroupMember></ZoneGroup>
            </ZoneGroups>"#;
        let escaped = state
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;");
        format!(
            r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><u:GetZoneGroupStateResponse xmlns:u="urn:schemas-upnp-org:service:ZoneGroupTopology:1"><ZoneGroupState>{escaped}</ZoneGroupState></u:GetZoneGroupStateResponse></s:Body></s:Envelope>"#
        )
    }

    fn sonos_household(uuid: &'static str) -> Option<Found> {
        let address = serve(move |head, _| {
            if head.starts_with("POST /zgt") {
                (200, None, topology_reply())
            } else {
                (200, None, sonos(uuid))
            }
        });
        describe(&format!("http://{address}/desc.xml"), address)
    }

    #[test]
    fn a_sonos_room_is_listed_under_the_name_of_its_group() {
        assert_eq!(
            speaker(sonos_household("RINCON_KITCHEN01400")).name,
            "Kitchen + Bedroom"
        );
        assert_eq!(speaker(sonos_household("RINCON_DEN01400")).name, "Den");
    }

    #[test]
    fn a_bonded_or_grouped_sonos_speaker_is_not_listed_on_its_own() {
        assert_eq!(
            sonos_household("RINCON_DENR01400"),
            Some(Found::Follower("upnp:RINCON_DENR01400".into()))
        );
        assert_eq!(
            sonos_household("RINCON_BEDROOM01400"),
            Some(Found::Follower("upnp:RINCON_BEDROOM01400".into()))
        );
        assert_eq!(
            sonos_household("RINCON_SUB01400"),
            Some(Found::Follower("upnp:RINCON_SUB01400".into())),
            "a sub of a home theater"
        );
    }

    #[test]
    fn a_sonos_speaker_that_will_not_say_how_it_is_arranged_is_listed_by_its_room() {
        let address = serve(|head, _| {
            if head.starts_with("POST /zgt") {
                (500, None, String::new())
            } else {
                (200, None, sonos("RINCON_ODD01400"))
            }
        });
        let device = speaker(describe(&format!("http://{address}/desc.xml"), address));
        assert_eq!(device.name, "Own room");
    }
}

//! Speakers that take a link and play it: Sonos and other UPnP / DLNA media renderers, driven with
//! SOAP over HTTP (the AVTransport and RenderingControl services).

use std::net::IpAddr;
use std::thread;
use std::time::{Duration, Instant};

use crate::device::{Device, Endpoint, Kind, Service, UpnpEndpoint};
use crate::media::{Media, Receiver, State, Status};
use crate::{Error, Result};

/// What discovery asks the network for.
pub(crate) const SEARCH_TARGET: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";
pub(crate) const SSDP_GROUP: &str = "239.255.255.250:1900";

const AV_TRANSPORT: &str = "urn:schemas-upnp-org:service:AVTransport:";
const RENDERING_CONTROL: &str = "urn:schemas-upnp-org:service:RenderingControl:";
const ZONE_TOPOLOGY: &str = "urn:schemas-upnp-org:service:ZoneGroupTopology:";

/// How long one request to a speaker may take. A speaker that takes longer is not coming back.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the speaker is really asked where it is; in between, the position is worked out.
const POLL_EVERY: Duration = Duration::from_millis(900);
/// How often its volume is read again, so the slider follows the speaker's own buttons and app.
const VOLUME_EVERY: Duration = Duration::from_secs(5);
/// How long `load` waits for a speaker that was told to play to say it is, before seeking.
const START_WAIT: Duration = Duration::from_secs(8);
const START_POLL: Duration = Duration::from_millis(250);

/// UPnP error codes that mean the speaker does not do what was asked, rather than that it failed.
const INVALID_ACTION: u32 = 401;
const NOT_IMPLEMENTED: u32 = 602;
const TRANSITION_NOT_AVAILABLE: u32 = 701;

/// An HTTP client for speakers. It follows no redirects: a speaker has no business sending this app
/// to another address, and what a redirect would reach is not for a speaker to say.
pub(crate) fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_global(Some(timeout))
        .build()
        .into()
}

/// The `M-SEARCH` that renderers answer.
pub(crate) fn search_request() -> String {
    format!(
        "M-SEARCH * HTTP/1.1\r\nHOST: {SSDP_GROUP}\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {SEARCH_TARGET}\r\nUSER-AGENT: Unsub\r\n\r\n"
    )
}

/// A renderer's answer to the search: where its description is.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SearchReply {
    pub location: String,
}

pub(crate) fn parse_search_reply(text: &str) -> Option<SearchReply> {
    let mut lines = text.lines();
    if !lines.next()?.contains(" 200 ") {
        return None;
    }
    let header = |wanted: &str| {
        text.lines().skip(1).find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case(wanted)
                .then(|| value.trim().to_owned())
        })
    };
    let location = header("location").filter(|location| location.starts_with("http://"))?;
    Some(SearchReply { location })
}

/// Makes `path` absolute against `base`, a URL; a path that already is one is left alone.
fn resolve(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_owned();
    }
    let origin = base.split_once("://").map_or(base, |(scheme, rest)| {
        let end = rest
            .find('/')
            .map_or(base.len(), |slash| scheme.len() + 3 + slash);
        &base[..end]
    });
    let origin = origin.trim_end_matches('/');
    if path.starts_with('/') {
        format!("{origin}{path}")
    } else {
        format!("{origin}/{path}")
    }
}

/// The host in `url`, if it is an `http` URL.
fn host_of(url: &str) -> Option<&str> {
    url.strip_prefix("http://")?.split(['/', ':', '?']).next()
}

/// Whether `url` is on the machine at `address`. A description that points control at somewhere
/// else is either wrong or trying to have this app talk to a machine that is not the speaker.
fn is_on(url: &str, address: IpAddr) -> bool {
    host_of(url).and_then(|host| host.parse::<IpAddr>().ok()) == Some(address)
}

fn child_text<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|child| child.has_tag_name(name))
        .and_then(|child| child.text())
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// Reads a device description. `None` if it describes nothing that plays audio from a link.
///
/// Sonos nests its renderer inside a "ZonePlayer" root, so services are looked for anywhere in
/// the document, and its speaker's room name, not the root's "192.168.1.5 - Sonos Port - RINCON…",
/// is what it is called.
pub(crate) fn parse_description(xml: &str, location: &str, address: IpAddr) -> Option<Device> {
    let document = roxmltree::Document::parse(xml).ok()?;
    let root = document.root_element();
    let device = root.children().find(|child| child.has_tag_name("device"))?;
    let base = child_text(root, "URLBase").unwrap_or(location);

    let service = |prefix: &str| {
        document
            .descendants()
            .filter(|node| node.has_tag_name("service"))
            .find_map(|node| {
                let service_type = child_text(node, "serviceType")
                    .filter(|service_type| service_type.starts_with(prefix))?;
                let control_url = child_text(node, "controlURL")?;
                Some(Service {
                    service_type: service_type.to_owned(),
                    control_url: resolve(base, control_url),
                })
                .filter(|service| is_on(&service.control_url, address))
            })
    };
    let av_transport = service(AV_TRANSPORT)?;
    let rendering_control = service(RENDERING_CONTROL);
    let zone_topology = service(ZONE_TOPOLOGY);

    let sonos = child_text(device, "manufacturer")
        .is_some_and(|maker| maker.to_ascii_lowercase().contains("sonos"));
    let name = [
        sonos.then(|| child_text(device, "roomName")).flatten(),
        child_text(device, "friendlyName"),
        child_text(device, "modelName"),
    ]
    .into_iter()
    .flatten()
    .next()
    .map_or_else(|| address.to_string(), str::to_owned);
    let udn = child_text(device, "UDN")?;
    Some(Device {
        id: format!("upnp:{}", udn.strip_prefix("uuid:").unwrap_or(udn)),
        name,
        kind: if sonos { Kind::Sonos } else { Kind::Upnp },
        address,
        endpoint: Endpoint::Upnp(UpnpEndpoint {
            av_transport,
            rendering_control,
            zone_topology,
        }),
    })
}

/// What a Sonos speaker is in the household's arrangement of speakers.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// Takes the commands for its room or group. The name is the group's, as the Sonos app writes it:
    /// "Kitchen", or "Kitchen + Bedroom" when rooms are playing together.
    Primary(String),
    /// The other half of a stereo pair, a sub or a surround, which only play what their primary does;
    /// or a room that has joined another's group, whose commands are the other's to take.
    Follower,
}

/// Reads a `ZoneGroupState` for the speaker `uuid` (`RINCON_…`). `None` if it isn't in it.
///
/// A group is led by its coordinator; bonded speakers are marked `Invisible`. The speaker commands
/// go to is the group's coordinator, or its first visible member if the coordinator is itself
/// a hidden half of a pair. A home theater's sub and surrounds are not members of the group at all
/// but `Satellite`s inside the soundbar's member.
pub(crate) fn parse_zone_role(state: &str, uuid: &str) -> Option<Role> {
    let document = roxmltree::Document::parse(state).ok()?;
    if document
        .descendants()
        .any(|node| node.has_tag_name("Satellite") && node.attribute("UUID") == Some(uuid))
    {
        return Some(Role::Follower);
    }
    for group in document
        .descendants()
        .filter(|node| node.has_tag_name("ZoneGroup"))
    {
        let members: Vec<_> = group
            .children()
            .filter(|node| node.has_tag_name("ZoneGroupMember"))
            .collect();
        if !members
            .iter()
            .any(|member| member.attribute("UUID") == Some(uuid))
        {
            continue;
        }
        let visible = |member: &&roxmltree::Node| member.attribute("Invisible") != Some("1");
        let primary = members
            .iter()
            .filter(visible)
            .find(|member| member.attribute("UUID") == group.attribute("Coordinator"))
            .or_else(|| members.iter().find(visible));
        if primary.and_then(|member| member.attribute("UUID")) != Some(uuid) {
            return Some(Role::Follower);
        }
        let mut names: Vec<&str> = Vec::new();
        for name in members
            .iter()
            .filter(visible)
            .filter_map(|member| member.attribute("ZoneName"))
        {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        return Some(Role::Primary(names.join(" + ")));
    }
    None
}

/// Asks a Sonos speaker how the speakers are arranged, and what it is in that.
pub(crate) fn zone_role(device: &Device) -> Result<Option<Role>> {
    let Endpoint::Upnp(UpnpEndpoint {
        zone_topology: Some(service),
        ..
    }) = &device.endpoint
    else {
        return Ok(None);
    };
    let reply = soap(&agent(REQUEST_TIMEOUT), service, "GetZoneGroupState", &[])?
        .map_err(|fault| refused("GetZoneGroupState", &fault))?;
    let uuid = device.id.strip_prefix("upnp:").unwrap_or(&device.id);
    Ok(value(&reply, "ZoneGroupState").and_then(|state| parse_zone_role(&state, uuid)))
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            // XML 1.0 has no way to write most control characters, and tags sometimes carry them.
            c if c.is_control() && !matches!(c, '\t' | '\n' | '\r') => {}
            c => escaped.push(c),
        }
    }
    escaped
}

/// `H:MM:SS`, which is how UPnP writes times; `fraction` adds the milliseconds.
fn clock(ms: u64, fraction: bool) -> String {
    let seconds = ms / 1000;
    let base = format!(
        "{}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    if fraction {
        format!("{base}.{:03}", ms % 1000)
    } else {
        base
    }
}

/// Reads `H:MM:SS` with optional fractional seconds. Speakers that cannot say write words such
/// as `NOT_IMPLEMENTED` instead.
fn parse_clock(text: &str) -> Option<u64> {
    let mut parts = text.trim().split(':');
    let (hours, minutes, seconds) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let seconds: f64 = seconds.parse().ok()?;
    let whole = hours.parse::<u64>().ok()? * 3600 + minutes.parse::<u64>().ok()? * 60;
    Some(whole * 1000 + (seconds * 1000.0).round() as u64)
}

/// The song described the way UPnP renderers show it (DIDL-Lite).
pub(crate) fn didl(media: &Media) -> String {
    let mut item = format!(
        "<dc:title>{}</dc:title><upnp:class>object.item.audioItem.musicTrack</upnp:class>",
        escape(&media.title)
    );
    if !media.artist.is_empty() {
        let artist = escape(&media.artist);
        item.push_str(&format!(
            "<dc:creator>{artist}</dc:creator><upnp:artist>{artist}</upnp:artist>"
        ));
    }
    if !media.album.is_empty() {
        item.push_str(&format!(
            "<upnp:album>{}</upnp:album>",
            escape(&media.album)
        ));
    }
    if let Some(art) = &media.art_url {
        item.push_str(&format!(
            "<upnp:albumArtURI>{}</upnp:albumArtURI>",
            escape(art)
        ));
    }
    let duration = if media.duration_ms > 0 {
        format!(" duration=\"{}\"", clock(media.duration_ms, true))
    } else {
        String::new()
    };
    item.push_str(&format!(
        "<res protocolInfo=\"http-get:*:{}:*\"{duration}>{}</res>",
        escape(&media.mime),
        escape(&media.url)
    ));
    format!(
        "<DIDL-Lite xmlns=\"urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\" \
         xmlns:upnp=\"urn:schemas-upnp-org:metadata-1-0/upnp/\"><item id=\"0\" parentID=\"-1\" restricted=\"1\">{item}</item></DIDL-Lite>"
    )
}

/// The body of a request for `action` of the service of type `service_type`. Arguments are in the
/// order the service lists them, which some speakers insist on.
pub(crate) fn envelope(service_type: &str, action: &str, arguments: &[(&str, &str)]) -> String {
    let arguments: String = arguments
        .iter()
        .map(|(name, value)| format!("<{name}>{}</{name}>", escape(value)))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\">\
         <s:Body><u:{action} xmlns:u=\"{service_type}\">{arguments}</u:{action}></s:Body></s:Envelope>"
    )
}

/// What a speaker says when it will not do something.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Fault {
    pub code: u32,
    pub description: String,
}

pub(crate) fn parse_fault(xml: &str) -> Option<Fault> {
    let document = roxmltree::Document::parse(xml).ok()?;
    let error = document
        .descendants()
        .find(|node| node.has_tag_name("UPnPError"))?;
    Some(Fault {
        code: child_text(error, "errorCode")?.parse().ok()?,
        description: child_text(error, "errorDescription")
            .unwrap_or_default()
            .to_owned(),
    })
}

/// The text of the first element called `name`, wherever it is.
fn value(xml: &str, name: &str) -> Option<String> {
    let document = roxmltree::Document::parse(xml).ok()?;
    let found = document
        .descendants()
        .find(|node| node.has_tag_name(name))?;
    Some(found.text().unwrap_or_default().trim().to_owned())
}

fn state_of(transport_state: &str) -> State {
    match transport_state {
        "PLAYING" => State::Playing,
        "PAUSED_PLAYBACK" | "PAUSED_RECORDING" => State::Paused,
        "TRANSITIONING" => State::Buffering,
        _ => State::Stopped,
    }
}

/// Sends one request. A speaker that refuses is an `Ok(Err(fault))`, so callers can tell a refusal
/// they expect from a speaker that is gone.
fn soap(
    agent: &ureq::Agent,
    service: &Service,
    action: &str,
    arguments: &[(&str, &str)],
) -> Result<std::result::Result<String, Fault>> {
    let body = envelope(&service.service_type, action, arguments);
    let mut response = agent
        .post(&service.control_url)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header(
            "SOAPACTION",
            format!("\"{}#{action}\"", service.service_type),
        )
        .send(body)?;
    let status = response.status().as_u16();
    let text = response.body_mut().read_to_string()?;
    if status == 200 {
        return Ok(Ok(text));
    }
    let fault = parse_fault(&text).unwrap_or(Fault {
        code: 0,
        description: format!("HTTP status {status}"),
    });
    Ok(Err(fault))
}

pub(crate) struct Renderer {
    agent: ureq::Agent,
    transport: Service,
    rendering: Option<Service>,
    volume: Option<f32>,
    /// When the volume was last read or set.
    volume_at: Instant,
    /// When the speaker was last asked, and what it said.
    polled: Option<(Instant, Status)>,
}

impl Renderer {
    /// Opens a connection and reads the volume, which is also how a speaker that has gone since it
    /// was found is noticed: asking is the first the speaker hears of us.
    pub(crate) fn connect(endpoint: &UpnpEndpoint) -> Result<Self> {
        let mut renderer = Self {
            agent: agent(REQUEST_TIMEOUT),
            transport: endpoint.av_transport.clone(),
            rendering: endpoint.rendering_control.clone(),
            volume: None,
            volume_at: Instant::now(),
            polled: None,
        };
        renderer.volume = match renderer.read_volume() {
            Ok(volume) => volume,
            Err(error) if error.is_lost() => return Err(error),
            // A speaker without a readable volume still plays; the slider just has nothing to start from.
            Err(_) => None,
        };
        Ok(renderer)
    }

    fn call(
        &self,
        service: &Service,
        action: &str,
        arguments: &[(&str, &str)],
    ) -> Result<std::result::Result<String, Fault>> {
        soap(&self.agent, service, action, arguments)
    }

    fn transport(&self, action: &str, arguments: &[(&str, &str)]) -> Result<String> {
        let mut with_instance = vec![("InstanceID", "0")];
        with_instance.extend_from_slice(arguments);
        self.call(&self.transport, action, &with_instance)?
            .map_err(|fault| refused(action, &fault))
    }

    fn read_volume(&self) -> Result<Option<f32>> {
        let Some(service) = &self.rendering else {
            return Ok(None);
        };
        let Ok(reply) = self.call(
            service,
            "GetVolume",
            &[("InstanceID", "0"), ("Channel", "Master")],
        )?
        else {
            return Ok(None);
        };
        Ok(value(&reply, "CurrentVolume")
            .and_then(|volume| volume.parse::<f32>().ok())
            .map(|volume| (volume / 100.0).clamp(0.0, 1.0)))
    }

    fn transport_state(&self) -> Result<State> {
        let reply = self.transport("GetTransportInfo", &[])?;
        Ok(state_of(
            &value(&reply, "CurrentTransportState").unwrap_or_default(),
        ))
    }

    /// Waits for a speaker told to play to be playing, which it must be before it can be seeked.
    fn wait_until_playing(&self) -> Result<()> {
        let deadline = Instant::now() + START_WAIT;
        while Instant::now() < deadline {
            if self.transport_state()? == State::Playing {
                return Ok(());
            }
            thread::sleep(START_POLL);
        }
        Ok(())
    }

    fn seek_to(&self, position_ms: u64) -> Result<()> {
        let target = clock(position_ms, false);
        self.transport("Seek", &[("Unit", "REL_TIME"), ("Target", &target)])
            .map(drop)
    }

    fn poll(&mut self) -> Result<Status> {
        if self.volume_at.elapsed() >= VOLUME_EVERY {
            self.volume_at = Instant::now();
            match self.read_volume() {
                Ok(Some(volume)) => self.volume = Some(volume),
                Err(error) if error.is_lost() => return Err(error),
                // A speaker that won't say keeps the volume it was last known at.
                Ok(None) | Err(_) => {}
            }
        }
        let state = self.transport_state()?;
        let info = self.transport("GetPositionInfo", &[])?;
        let status = Status {
            state,
            position_ms: value(&info, "RelTime")
                .and_then(|time| parse_clock(&time))
                .unwrap_or(0),
            duration_ms: value(&info, "TrackDuration")
                .and_then(|time| parse_clock(&time))
                .unwrap_or(0),
            volume: self.volume,
            url: value(&info, "TrackURI").filter(|uri| !uri.is_empty()),
            finished: false,
            error: None,
        };
        self.polled = Some((Instant::now(), status.clone()));
        Ok(status)
    }
}

fn refused(action: &str, fault: &Fault) -> Error {
    Error::Refused(format!(
        "the speaker refused {action}: {} ({})",
        fault.description, fault.code
    ))
}

impl Receiver for Renderer {
    fn load(&mut self, media: &Media, start_ms: u64, autoplay: bool) -> Result<()> {
        self.polled = None;
        self.transport(
            "SetAVTransportURI",
            &[
                ("CurrentURI", &media.url),
                ("CurrentURIMetaData", &didl(media)),
            ],
        )?;
        self.transport("Play", &[("Speed", "1")])?;
        if start_ms > 0 {
            self.wait_until_playing()?;
            // Starting somewhere else is a nicety: a speaker that can't is better left playing from the top.
            match self.seek_to(start_ms) {
                Err(error) if error.is_lost() => return Err(error),
                _ => {}
            }
        }
        if !autoplay {
            self.wait_until_playing()?;
            self.transport("Pause", &[])?;
        }
        Ok(())
    }

    fn preload(&mut self, media: &Media) -> Result<bool> {
        let reply = self.call(
            &self.transport,
            "SetNextAVTransportURI",
            &[
                ("InstanceID", "0"),
                ("NextURI", &media.url),
                ("NextURIMetaData", &didl(media)),
            ],
        )?;
        match reply {
            Ok(_) => Ok(true),
            // Queueing is optional for renderers; one without it has the next song loaded when this one ends.
            Err(Fault {
                code: INVALID_ACTION | NOT_IMPLEMENTED | TRANSITION_NOT_AVAILABLE,
                ..
            }) => Ok(false),
            Err(fault) => Err(refused("SetNextAVTransportURI", &fault)),
        }
    }

    fn clear_preload(&mut self) -> Result<()> {
        self.transport(
            "SetNextAVTransportURI",
            &[("NextURI", ""), ("NextURIMetaData", "")],
        )
        .map(drop)
    }

    fn play(&mut self) -> Result<()> {
        self.polled = None;
        self.transport("Play", &[("Speed", "1")]).map(drop)
    }

    fn pause(&mut self) -> Result<()> {
        self.polled = None;
        self.transport("Pause", &[]).map(drop)
    }

    fn seek(&mut self, position_ms: u64) -> Result<()> {
        self.polled = None;
        self.seek_to(position_ms)
    }

    fn set_volume(&mut self, volume: f32) -> Result<()> {
        let Some(service) = self.rendering.clone() else {
            return Ok(());
        };
        let percent = (volume.clamp(0.0, 1.0) * 100.0).round() as u32;
        self.call(
            &service,
            "SetVolume",
            &[
                ("InstanceID", "0"),
                ("Channel", "Master"),
                ("DesiredVolume", &percent.to_string()),
            ],
        )?
        .map_err(|fault| refused("SetVolume", &fault))?;
        self.volume = Some(volume.clamp(0.0, 1.0));
        self.volume_at = Instant::now();
        Ok(())
    }

    fn status(&mut self) -> Result<Status> {
        if let Some((at, status)) = &self.polled
            && at.elapsed() < POLL_EVERY
        {
            return Ok(Status {
                volume: self.volume,
                ..status.advanced(at.elapsed())
            });
        }
        self.poll()
    }

    fn stop(&mut self) -> Result<()> {
        self.polled = None;
        self.transport("Stop", &[]).map(drop)
    }

    fn disconnect(&mut self) {}
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener};
    use std::sync::{Arc, Mutex};

    use super::*;

    const SONOS: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <device>
    <deviceType>urn:schemas-upnp-org:device:ZonePlayer:1</deviceType>
    <friendlyName>192.168.1.20 - Sonos Play:1 - RINCON_B8E937A1B2C301400</friendlyName>
    <manufacturer>Sonos, Inc.</manufacturer>
    <modelName>Sonos Play:1</modelName>
    <UDN>uuid:RINCON_B8E937A1B2C301400</UDN>
    <roomName>Kitchen</roomName>
    <deviceList>
      <device>
        <deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType>
        <UDN>uuid:RINCON_B8E937A1B2C301400_MS</UDN>
        <serviceList><service><serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType><controlURL>/MediaServer/ContentDirectory/Control</controlURL></service></serviceList>
      </device>
      <device>
        <deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType>
        <UDN>uuid:RINCON_B8E937A1B2C301400_MR</UDN>
        <serviceList>
          <service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType><controlURL>/MediaRenderer/RenderingControl/Control</controlURL></service>
          <service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>/MediaRenderer/AVTransport/Control</controlURL></service>
        </serviceList>
      </device>
    </deviceList>
  </device>
</root>"#;

    const GENERIC: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <URLBase>http://192.168.1.30:49152/</URLBase>
  <device>
    <deviceType>urn:schemas-upnp-org:device:MediaRenderer:1</deviceType>
    <friendlyName>Living Room Amp</friendlyName>
    <manufacturer>Acme Audio</manufacturer>
    <modelName>AmpStream 3</modelName>
    <UDN>uuid:2f402f80-da50-11e1-9b23-001788102201</UDN>
    <serviceList>
      <service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>upnp/control/avt</controlURL></service>
      <service><serviceType>urn:schemas-upnp-org:service:RenderingControl:1</serviceType><controlURL>http://192.168.1.30:49152/upnp/control/rc</controlURL></service>
    </serviceList>
  </device>
</root>"#;

    fn address(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn song() -> Media {
        Media {
            url: "http://192.168.1.2:5000/link/abc.flac".into(),
            mime: "audio/flac".into(),
            title: "Rock & Roll <3".into(),
            artist: "The \"Band\"".into(),
            album: "Hits".into(),
            art_url: Some("http://192.168.1.2:5000/link/art.jpg".into()),
            duration_ms: 205_000,
        }
    }

    #[test]
    fn the_search_asks_for_media_renderers() {
        let request = search_request();
        assert!(request.starts_with("M-SEARCH * HTTP/1.1\r\n"));
        assert!(request.contains("MAN: \"ssdp:discover\"\r\n"));
        assert!(request.contains("ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n"));
        assert!(request.ends_with("\r\n\r\n"));
    }

    #[test]
    fn a_search_reply_gives_the_location_whatever_the_case_of_its_headers() {
        let reply = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age = 1800\r\nLocation: http://192.168.1.20:1400/xml/device_description.xml\r\nST: urn:schemas-upnp-org:device:MediaRenderer:1\r\nUSN: uuid:RINCON_1::urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n";
        assert_eq!(
            parse_search_reply(reply),
            Some(SearchReply {
                location: "http://192.168.1.20:1400/xml/device_description.xml".into()
            })
        );
    }

    #[test]
    fn announcements_and_replies_without_a_location_are_not_search_replies() {
        assert_eq!(
            parse_search_reply(
                "NOTIFY * HTTP/1.1\r\nLOCATION: http://192.168.1.20:1400/d.xml\r\n\r\n"
            ),
            None
        );
        assert_eq!(parse_search_reply("HTTP/1.1 200 OK\r\nST: x\r\n\r\n"), None);
        assert_eq!(
            parse_search_reply("HTTP/1.1 200 OK\r\nLOCATION: file:///etc/passwd\r\n\r\n"),
            None
        );
        assert_eq!(parse_search_reply(""), None);
    }

    #[test]
    fn sonos_is_found_by_its_maker_and_named_by_its_room() {
        let device = parse_description(
            SONOS,
            "http://192.168.1.20:1400/xml/device_description.xml",
            address("192.168.1.20"),
        )
        .unwrap();
        assert_eq!(
            (device.kind, device.name.as_str()),
            (Kind::Sonos, "Kitchen")
        );
        assert_eq!(device.id, "upnp:RINCON_B8E937A1B2C301400");
        let Endpoint::Upnp(endpoint) = &device.endpoint else {
            panic!("not UPnP")
        };
        assert_eq!(
            endpoint.av_transport.control_url,
            "http://192.168.1.20:1400/MediaRenderer/AVTransport/Control"
        );
        assert_eq!(
            endpoint.av_transport.service_type,
            "urn:schemas-upnp-org:service:AVTransport:1"
        );
        assert_eq!(
            endpoint.rendering_control.as_ref().unwrap().control_url,
            "http://192.168.1.20:1400/MediaRenderer/RenderingControl/Control"
        );
    }

    #[test]
    fn another_renderer_keeps_its_friendly_name_and_resolves_urls_against_its_base() {
        let device = parse_description(
            GENERIC,
            "http://192.168.1.30:49152/desc.xml",
            address("192.168.1.30"),
        )
        .unwrap();
        assert_eq!(
            (device.kind, device.name.as_str()),
            (Kind::Upnp, "Living Room Amp")
        );
        assert_eq!(device.id, "upnp:2f402f80-da50-11e1-9b23-001788102201");
        let Endpoint::Upnp(endpoint) = &device.endpoint else {
            panic!("not UPnP")
        };
        assert_eq!(
            endpoint.av_transport.control_url,
            "http://192.168.1.30:49152/upnp/control/avt"
        );
        assert_eq!(
            endpoint.rendering_control.as_ref().unwrap().control_url,
            "http://192.168.1.30:49152/upnp/control/rc"
        );
    }

    #[test]
    fn a_device_that_cannot_play_a_link_is_not_a_speaker() {
        let router = r#"<root xmlns="urn:schemas-upnp-org:device-1-0"><device><friendlyName>Router</friendlyName><UDN>uuid:1</UDN></device></root>"#;
        assert_eq!(
            parse_description(router, "http://192.168.1.1/d.xml", address("192.168.1.1")),
            None
        );
        assert_eq!(
            parse_description(
                "not xml",
                "http://192.168.1.1/d.xml",
                address("192.168.1.1")
            ),
            None
        );
    }

    #[test]
    fn a_renderer_without_a_name_is_called_by_its_address() {
        let xml = r#"<root><device><UDN>uuid:9</UDN><serviceList><service><serviceType>urn:schemas-upnp-org:service:AVTransport:1</serviceType><controlURL>/c</controlURL></service></serviceList></device></root>"#;
        assert_eq!(
            parse_description(xml, "http://192.168.1.9:80/d.xml", address("192.168.1.9"))
                .unwrap()
                .name,
            "192.168.1.9"
        );
    }

    #[test]
    fn urls_resolve_against_the_origin() {
        assert_eq!(resolve("http://h:1/a/b.xml", "/c"), "http://h:1/c");
        assert_eq!(resolve("http://h:1/a/b.xml", "c"), "http://h:1/c");
        assert_eq!(resolve("http://h:1", "/c"), "http://h:1/c");
        assert_eq!(resolve("http://h:1/", "http://x/y"), "http://x/y");
    }

    #[test]
    fn an_envelope_names_the_service_and_the_action_and_orders_its_arguments() {
        let body = envelope(
            "urn:schemas-upnp-org:service:AVTransport:1",
            "Seek",
            &[
                ("InstanceID", "0"),
                ("Unit", "REL_TIME"),
                ("Target", "0:01:05"),
            ],
        );
        assert_eq!(
            body,
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
             s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:Seek xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\">\
             <InstanceID>0</InstanceID><Unit>REL_TIME</Unit><Target>0:01:05</Target></u:Seek></s:Body></s:Envelope>"
        );
        assert!(roxmltree::Document::parse(&body).is_ok());
    }

    #[test]
    fn didl_escapes_what_a_title_may_contain_and_describes_the_song() {
        let didl = didl(&song());
        let document = roxmltree::Document::parse(&didl).expect("well-formed DIDL-Lite");
        let text = |name: &str| {
            document
                .descendants()
                .find(|n| n.has_tag_name(name))
                .and_then(|n| n.text())
                .unwrap()
                .to_owned()
        };
        assert_eq!(text("title"), "Rock & Roll <3");
        assert_eq!(text("artist"), "The \"Band\"");
        assert_eq!(text("album"), "Hits");
        assert_eq!(text("albumArtURI"), "http://192.168.1.2:5000/link/art.jpg");
        assert_eq!(text("class"), "object.item.audioItem.musicTrack");
        let res = document
            .descendants()
            .find(|n| n.has_tag_name("res"))
            .unwrap();
        assert_eq!(
            res.attribute("protocolInfo"),
            Some("http-get:*:audio/flac:*")
        );
        assert_eq!(res.attribute("duration"), Some("0:03:25.000"));
        assert_eq!(res.text(), Some("http://192.168.1.2:5000/link/abc.flac"));
        assert!(didl.contains("Rock &amp; Roll &lt;3"));
    }

    #[test]
    fn didl_inside_a_request_is_escaped_a_second_time() {
        let body = envelope(
            "urn:schemas-upnp-org:service:AVTransport:1",
            "SetAVTransportURI",
            &[("CurrentURIMetaData", &didl(&song()))],
        );
        assert!(body.contains("Rock &amp;amp; Roll &amp;lt;3"));
        let document = roxmltree::Document::parse(&body).unwrap();
        let metadata = document
            .descendants()
            .find(|n| n.has_tag_name("CurrentURIMetaData"))
            .and_then(|n| n.text())
            .unwrap();
        assert_eq!(metadata, didl(&song()));
    }

    #[test]
    fn control_characters_in_tags_are_dropped_rather_than_breaking_the_document() {
        assert_eq!(escape("a\u{0}b\u{1b}c\td"), "abc\td");
    }

    #[test]
    fn didl_leaves_out_what_the_song_does_not_have() {
        let bare = Media {
            artist: String::new(),
            album: String::new(),
            art_url: None,
            duration_ms: 0,
            ..song()
        };
        let didl = didl(&bare);
        for absent in ["creator", "artist", "album", "albumArtURI", "duration"] {
            assert!(!didl.contains(absent), "{absent}");
        }
    }

    #[test]
    fn times_are_written_and_read_the_upnp_way() {
        assert_eq!(clock(65_000, false), "0:01:05");
        assert_eq!(clock(3_725_250, true), "1:02:05.250");
        assert_eq!(parse_clock("0:03:25"), Some(205_000));
        assert_eq!(parse_clock("1:02:05.250"), Some(3_725_250));
        assert_eq!(parse_clock("NOT_IMPLEMENTED"), None);
        assert_eq!(parse_clock("0:03"), None);
    }

    #[test]
    fn a_fault_gives_its_code_and_words() {
        let xml = r#"<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/"><s:Body><s:Fault><faultcode>s:Client</faultcode><faultstring>UPnPError</faultstring>
            <detail><UPnPError xmlns="urn:schemas-upnp-org:control-1-0"><errorCode>714</errorCode><errorDescription>Illegal MIME-Type</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>"#;
        assert_eq!(
            parse_fault(xml),
            Some(Fault {
                code: 714,
                description: "Illegal MIME-Type".into()
            })
        );
        assert_eq!(parse_fault("<ok/>"), None);
    }

    #[test]
    fn transport_states_map_onto_the_players() {
        assert_eq!(state_of("PLAYING"), State::Playing);
        assert_eq!(state_of("PAUSED_PLAYBACK"), State::Paused);
        assert_eq!(state_of("TRANSITIONING"), State::Buffering);
        assert_eq!(state_of("STOPPED"), State::Stopped);
        assert_eq!(state_of("NO_MEDIA_PRESENT"), State::Stopped);
    }

    /// A renderer on a local socket: it keeps every request it is sent and answers each action from
    /// `reply`, which gets the action and the request body and gives an HTTP status and a body.
    struct FakeRenderer {
        requests: Arc<Mutex<Vec<(String, String)>>>,
        endpoint: UpnpEndpoint,
    }

    type Reply = Box<dyn Fn(&str, &str) -> (u16, String) + Send>;

    impl FakeRenderer {
        fn start(reply: Reply) -> Self {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let requests: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
            let seen = Arc::clone(&requests);
            thread::spawn(move || {
                for connection in listener.incoming() {
                    let Ok(mut connection) = connection else {
                        return;
                    };
                    let mut buffered = Vec::new();
                    let mut chunk = [0_u8; 4096];
                    let (head, body) = loop {
                        let Ok(read) = connection.read(&mut chunk) else {
                            break (String::new(), String::new());
                        };
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
                    let action = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .starts_with("soapaction:")
                                .then(|| {
                                    line.split('#')
                                        .nth(1)
                                        .unwrap_or("")
                                        .trim_end_matches('"')
                                        .to_owned()
                                })
                        })
                        .unwrap_or_default();
                    let (status, response) = reply(&action, &body);
                    seen.lock().unwrap().push((action, body));
                    let _ = write!(
                        connection,
                        "HTTP/1.1 {status} X\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                        response.len()
                    );
                }
            });
            let service = |name: &str, path: &str| Service {
                service_type: format!("urn:schemas-upnp-org:service:{name}:1"),
                control_url: format!("{base}{path}"),
            };
            Self {
                requests,
                endpoint: UpnpEndpoint {
                    av_transport: service("AVTransport", "/avt"),
                    rendering_control: Some(service("RenderingControl", "/rc")),
                    zone_topology: None,
                },
            }
        }

        fn actions(&self) -> Vec<String> {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .map(|(action, _)| action.clone())
                .collect()
        }

        fn body_of(&self, action: &str) -> String {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .find(|(name, _)| name == action)
                .map(|(_, body)| body.clone())
                .unwrap_or_default()
        }
    }

    fn response(action: &str, fields: &str) -> String {
        format!(
            "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body>\
             <u:{action}Response xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\">{fields}</u:{action}Response></s:Body></s:Envelope>"
        )
    }

    fn fault(code: u32) -> (u16, String) {
        (
            500,
            format!(
                "<s:Envelope xmlns:s=\"x\"><s:Body><s:Fault><detail><UPnPError><errorCode>{code}</errorCode><errorDescription>No</errorDescription></UPnPError></detail></s:Fault></s:Body></s:Envelope>"
            ),
        )
    }

    #[test]
    fn loading_a_song_sets_it_plays_it_and_seeks_once_playing() {
        let fake = FakeRenderer::start(Box::new(|action, _| match action {
            "GetVolume" => (200, response(action, "<CurrentVolume>35</CurrentVolume>")),
            "GetTransportInfo" => (
                200,
                response(
                    action,
                    "<CurrentTransportState>PLAYING</CurrentTransportState>",
                ),
            ),
            _ => (200, response(action, "")),
        }));
        let mut renderer = Renderer::connect(&fake.endpoint).unwrap();
        assert_eq!(renderer.volume, Some(0.35));

        renderer.load(&song(), 65_000, true).unwrap();

        assert_eq!(
            fake.actions(),
            [
                "GetVolume",
                "SetAVTransportURI",
                "Play",
                "GetTransportInfo",
                "Seek"
            ]
        );
        let set = fake.body_of("SetAVTransportURI");
        assert!(set.contains("<CurrentURI>http://192.168.1.2:5000/link/abc.flac</CurrentURI>"));
        assert!(
            set.contains("Rock &amp;amp; Roll"),
            "the song's details travel as DIDL-Lite"
        );
        let seek = fake.body_of("Seek");
        assert!(seek.contains("<Unit>REL_TIME</Unit><Target>0:01:05</Target>"));
    }

    #[test]
    fn a_song_loaded_paused_is_played_just_long_enough_to_pause_it() {
        let fake = FakeRenderer::start(Box::new(|action, _| match action {
            "GetTransportInfo" => (
                200,
                response(
                    action,
                    "<CurrentTransportState>PLAYING</CurrentTransportState>",
                ),
            ),
            _ => (200, response(action, "")),
        }));
        let mut renderer = Renderer::connect(&fake.endpoint).unwrap();
        renderer.load(&song(), 0, false).unwrap();
        let actions = fake.actions();
        assert_eq!(&actions[actions.len() - 2..], ["GetTransportInfo", "Pause"]);
    }

    #[test]
    fn a_speaker_that_cannot_seek_still_plays_the_song() {
        let fake = FakeRenderer::start(Box::new(|action, _| match action {
            "GetTransportInfo" => (
                200,
                response(
                    action,
                    "<CurrentTransportState>PLAYING</CurrentTransportState>",
                ),
            ),
            "Seek" => fault(701),
            _ => (200, response(action, "")),
        }));
        let mut renderer = Renderer::connect(&fake.endpoint).unwrap();
        renderer.load(&song(), 30_000, true).unwrap();
    }

    #[test]
    fn status_reads_the_state_position_and_what_is_loaded() {
        let fake = FakeRenderer::start(Box::new(|action, _| match action {
            "GetTransportInfo" => (
                200,
                response(
                    action,
                    "<CurrentTransportState>PAUSED_PLAYBACK</CurrentTransportState>",
                ),
            ),
            "GetPositionInfo" => (
                200,
                response(
                    action,
                    "<TrackDuration>0:03:25</TrackDuration><TrackURI>http://192.168.1.2:5000/link/abc.flac</TrackURI><RelTime>0:01:10</RelTime>",
                ),
            ),
            _ => (200, response(action, "")),
        }));
        let mut renderer = Renderer::connect(&fake.endpoint).unwrap();
        let status = renderer.status().unwrap();
        assert_eq!(status.state, State::Paused);
        assert_eq!((status.position_ms, status.duration_ms), (70_000, 205_000));
        assert_eq!(
            status.url.as_deref(),
            Some("http://192.168.1.2:5000/link/abc.flac")
        );
        // Asked again at once, it works the answer out instead of asking the speaker.
        let before = fake.actions().len();
        renderer.status().unwrap();
        assert_eq!(fake.actions().len(), before);
    }

    #[test]
    fn a_playing_status_advances_between_questions() {
        let fake = FakeRenderer::start(Box::new(|action, _| match action {
            "GetTransportInfo" => (
                200,
                response(
                    action,
                    "<CurrentTransportState>PLAYING</CurrentTransportState>",
                ),
            ),
            "GetPositionInfo" => (
                200,
                response(
                    action,
                    "<TrackDuration>0:03:25</TrackDuration><TrackURI>u</TrackURI><RelTime>0:00:10</RelTime>",
                ),
            ),
            _ => (200, response(action, "")),
        }));
        let mut renderer = Renderer::connect(&fake.endpoint).unwrap();
        let first = renderer.status().unwrap();
        thread::sleep(Duration::from_millis(120));
        assert!(renderer.status().unwrap().position_ms >= first.position_ms + 100);
    }

    #[test]
    fn volume_goes_out_as_a_percentage_of_the_master_channel() {
        let fake = FakeRenderer::start(Box::new(|action, _| (200, response(action, ""))));
        let mut renderer = Renderer::connect(&fake.endpoint).unwrap();
        renderer.set_volume(0.5).unwrap();
        let body = fake.body_of("SetVolume");
        assert!(body.contains("<Channel>Master</Channel><DesiredVolume>50</DesiredVolume>"));
        assert_eq!(renderer.volume, Some(0.5));
    }

    #[test]
    fn a_renderer_that_cannot_queue_says_so_and_one_that_can_does() {
        let cannot = FakeRenderer::start(Box::new(|action, _| {
            if action == "SetNextAVTransportURI" {
                fault(402)
            } else {
                (200, response(action, ""))
            }
        }));
        assert!(matches!(
            Renderer::connect(&cannot.endpoint)
                .unwrap()
                .preload(&song()),
            Err(Error::Refused(_))
        ));
        let unsupported = FakeRenderer::start(Box::new(|action, _| {
            if action == "SetNextAVTransportURI" {
                fault(602)
            } else {
                (200, response(action, ""))
            }
        }));
        assert!(
            !Renderer::connect(&unsupported.endpoint)
                .unwrap()
                .preload(&song())
                .unwrap()
        );
        let can = FakeRenderer::start(Box::new(|action, _| (200, response(action, ""))));
        assert!(
            Renderer::connect(&can.endpoint)
                .unwrap()
                .preload(&song())
                .unwrap()
        );
        assert!(
            can.body_of("SetNextAVTransportURI")
                .contains("<NextURI>http://192.168.1.2:5000/link/abc.flac</NextURI>")
        );
    }

    #[test]
    fn a_refusal_is_not_a_lost_speaker_but_silence_is() {
        let fake = FakeRenderer::start(Box::new(|action, _| {
            if action == "Pause" {
                fault(701)
            } else {
                (200, response(action, ""))
            }
        }));
        let mut renderer = Renderer::connect(&fake.endpoint).unwrap();
        let refusal = renderer.pause().unwrap_err();
        assert!(matches!(refusal, Error::Refused(_)) && !refusal.is_lost());

        let gone = UpnpEndpoint {
            av_transport: Service {
                service_type: "urn:schemas-upnp-org:service:AVTransport:1".into(),
                control_url: "http://127.0.0.1:1/avt".into(),
            },
            rendering_control: None,
            zone_topology: None,
        };
        assert!(
            Renderer::connect(&gone)
                .unwrap()
                .pause()
                .unwrap_err()
                .is_lost()
        );
    }

    #[test]
    fn a_speaker_that_has_gone_is_noticed_when_connecting_to_it() {
        let service = |name: &str, path: &str| Service {
            service_type: format!("urn:schemas-upnp-org:service:{name}:1"),
            control_url: format!("http://127.0.0.1:1{path}"),
        };
        let gone = UpnpEndpoint {
            av_transport: service("AVTransport", "/avt"),
            rendering_control: Some(service("RenderingControl", "/rc")),
            zone_topology: None,
        };
        assert!(Renderer::connect(&gone).err().unwrap().is_lost());
    }

    #[test]
    fn the_volume_is_read_again_now_and_then_to_follow_the_speakers_own_buttons() {
        let asked = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter = Arc::clone(&asked);
        let fake = FakeRenderer::start(Box::new(move |action, _| match action {
            "GetVolume" => {
                let times = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let level = if times == 0 { 20 } else { 60 };
                (
                    200,
                    response(action, &format!("<CurrentVolume>{level}</CurrentVolume>")),
                )
            }
            "GetTransportInfo" => (
                200,
                response(
                    action,
                    "<CurrentTransportState>PLAYING</CurrentTransportState>",
                ),
            ),
            "GetPositionInfo" => (
                200,
                response(
                    action,
                    "<TrackDuration>0:03:25</TrackDuration><TrackURI>u</TrackURI><RelTime>0:00:10</RelTime>",
                ),
            ),
            _ => (200, response(action, "")),
        }));
        let mut renderer = Renderer::connect(&fake.endpoint).unwrap();
        assert_eq!(renderer.status().unwrap().volume, Some(0.2));

        renderer.polled = None;
        renderer.volume_at = Instant::now() - VOLUME_EVERY;
        assert_eq!(
            renderer.status().unwrap().volume,
            Some(0.6),
            "the speaker was turned up by hand"
        );

        renderer.polled = None;
        renderer.status().unwrap();
        assert_eq!(
            asked.load(std::sync::atomic::Ordering::Relaxed),
            2,
            "and is not asked again at once"
        );
    }

    #[test]
    fn a_description_that_sends_control_elsewhere_is_not_a_speaker() {
        let moved = GENERIC.replace("http://192.168.1.30:49152/", "http://10.9.9.9:49152/");
        assert_eq!(
            parse_description(
                &moved,
                "http://192.168.1.30:49152/desc.xml",
                address("192.168.1.30")
            ),
            None
        );
        let sneaky = GENERIC.replace("upnp/control/avt", "http://10.9.9.9/avt");
        assert_eq!(
            parse_description(
                &sneaky,
                "http://192.168.1.30:49152/desc.xml",
                address("192.168.1.30")
            ),
            None
        );
    }

    #[test]
    fn a_service_elsewhere_is_dropped_but_the_speaker_stays() {
        let sneaky = GENERIC.replace(
            "http://192.168.1.30:49152/upnp/control/rc",
            "http://10.9.9.9/rc",
        );
        let device = parse_description(
            &sneaky,
            "http://192.168.1.30:49152/desc.xml",
            address("192.168.1.30"),
        )
        .unwrap();
        let Endpoint::Upnp(endpoint) = &device.endpoint else {
            panic!("not UPnP")
        };
        assert_eq!(endpoint.rendering_control, None);
    }

    #[test]
    fn only_http_urls_on_the_speakers_address_are_its_own() {
        let address = address("192.168.1.30");
        assert!(is_on("http://192.168.1.30:1400/x", address));
        assert!(is_on("http://192.168.1.30/x", address));
        assert!(!is_on("http://192.168.1.31:1400/x", address));
        assert!(!is_on("https://192.168.1.30/x", address));
        assert!(!is_on("http://speaker.local/x", address));
        assert!(!is_on("http://192.168.1.30.evil.example/x", address));
    }

    /// A household, in the shape a Sonos speaker reports it: a Living Room home theater (a soundbar
    /// with a sub and two surrounds, which are `Satellite`s inside the soundbar's member), a Den
    /// stereo pair (two members, the right one hidden), Kitchen and Bedroom playing together with
    /// Kitchen leading, and Office on its own.
    const HOUSEHOLD: &str = r#"<ZoneGroups>
        <ZoneGroup Coordinator="RINCON_ARC01400" ID="RINCON_ARC01400:11">
            <ZoneGroupMember UUID="RINCON_ARC01400" Location="http://192.168.1.20:1400/xml/device_description.xml" ZoneName="Living Room">
                <Satellite UUID="RINCON_SUB01400" Location="http://192.168.1.21:1400/xml/device_description.xml" Invisible="1" SatelliteCapabilities="3"/>
                <Satellite UUID="RINCON_SURROUNDL01400" Location="http://192.168.1.22:1400/xml/device_description.xml" Invisible="1" SatelliteCapabilities="16"/>
                <Satellite UUID="RINCON_SURROUNDR01400" Location="http://192.168.1.23:1400/xml/device_description.xml" Invisible="1" SatelliteCapabilities="16"/>
            </ZoneGroupMember>
        </ZoneGroup>
        <ZoneGroup Coordinator="RINCON_DENL01400" ID="RINCON_DENL01400:12">
            <ZoneGroupMember UUID="RINCON_DENL01400" Location="http://192.168.1.24:1400/xml/device_description.xml" ZoneName="Den"/>
            <ZoneGroupMember UUID="RINCON_DENR01400" Location="http://192.168.1.25:1400/xml/device_description.xml" ZoneName="Den" Invisible="1"/>
        </ZoneGroup>
        <ZoneGroup Coordinator="RINCON_KITCHEN01400" ID="RINCON_KITCHEN01400:13">
            <ZoneGroupMember UUID="RINCON_KITCHEN01400" Location="http://192.168.1.26:1400/xml/device_description.xml" ZoneName="Kitchen"/>
            <ZoneGroupMember UUID="RINCON_BEDROOM01400" Location="http://192.168.1.27:1400/xml/device_description.xml" ZoneName="Bedroom"/>
        </ZoneGroup>
        <ZoneGroup Coordinator="RINCON_OFFICE01400" ID="RINCON_OFFICE01400:14">
            <ZoneGroupMember UUID="RINCON_OFFICE01400" Location="http://192.168.1.28:1400/xml/device_description.xml" ZoneName="Office"/>
        </ZoneGroup>
        <VanishedDevices/>
    </ZoneGroups>"#;

    #[test]
    fn a_bonded_stereo_pair_is_listed_once_under_the_speaker_that_takes_its_commands() {
        let role = |uuid| parse_zone_role(HOUSEHOLD, uuid);
        assert_eq!(role("RINCON_DENL01400"), Some(Role::Primary("Den".into())));
        assert_eq!(role("RINCON_DENR01400"), Some(Role::Follower));
    }

    #[test]
    fn a_home_theaters_sub_and_surrounds_are_not_rooms_of_their_own() {
        let role = |uuid| parse_zone_role(HOUSEHOLD, uuid);
        assert_eq!(
            role("RINCON_ARC01400"),
            Some(Role::Primary("Living Room".into()))
        );
        for satellite in [
            "RINCON_SUB01400",
            "RINCON_SURROUNDL01400",
            "RINCON_SURROUNDR01400",
        ] {
            assert_eq!(role(satellite), Some(Role::Follower), "{satellite}");
        }
    }

    #[test]
    fn rooms_playing_together_are_one_speaker_named_for_all_of_them() {
        let role = |uuid| parse_zone_role(HOUSEHOLD, uuid);
        assert_eq!(
            role("RINCON_KITCHEN01400"),
            Some(Role::Primary("Kitchen + Bedroom".into()))
        );
        assert_eq!(role("RINCON_BEDROOM01400"), Some(Role::Follower));
        assert_eq!(
            role("RINCON_OFFICE01400"),
            Some(Role::Primary("Office".into()))
        );
    }

    #[test]
    fn a_group_whose_coordinator_is_a_hidden_half_is_led_by_its_first_visible_member() {
        let state = r#"<ZoneGroups><ZoneGroup Coordinator="RINCON_B01400">
            <ZoneGroupMember UUID="RINCON_A01400" ZoneName="Den"/>
            <ZoneGroupMember UUID="RINCON_B01400" ZoneName="Den" Invisible="1"/></ZoneGroup></ZoneGroups>"#;
        assert_eq!(
            parse_zone_role(state, "RINCON_A01400"),
            Some(Role::Primary("Den".into()))
        );
        assert_eq!(
            parse_zone_role(state, "RINCON_B01400"),
            Some(Role::Follower)
        );
    }

    #[test]
    fn a_speaker_the_topology_does_not_mention_has_no_role_and_garbage_has_none_either() {
        assert_eq!(parse_zone_role(HOUSEHOLD, "RINCON_STRANGER01400"), None);
        assert_eq!(parse_zone_role("not xml", "RINCON_OFFICE01400"), None);
    }

    #[test]
    fn the_topology_service_is_read_from_a_sonos_description() {
        let with_topology = SONOS.replace(
            "<deviceList>",
            "<serviceList><service><serviceType>urn:schemas-upnp-org:service:ZoneGroupTopology:1</serviceType><controlURL>/ZoneGroupTopology/Control</controlURL></service></serviceList><deviceList>",
        );
        let device = parse_description(
            &with_topology,
            "http://192.168.1.20:1400/xml/device_description.xml",
            address("192.168.1.20"),
        )
        .unwrap();
        let Endpoint::Upnp(endpoint) = &device.endpoint else {
            panic!("not UPnP")
        };
        assert_eq!(
            endpoint.zone_topology.as_ref().unwrap().control_url,
            "http://192.168.1.20:1400/ZoneGroupTopology/Control"
        );
    }
}

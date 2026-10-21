use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

/// This machine's address on the interface that reaches `speaker`: the one to serve audio from,
/// and the only one to listen on. Asks the routing table with a connected UDP socket, which sends
/// nothing.
pub fn local_address_toward(speaker: IpAddr) -> Option<IpAddr> {
    let wildcard: IpAddr = if speaker.is_ipv4() {
        Ipv4Addr::UNSPECIFIED.into()
    } else {
        Ipv6Addr::UNSPECIFIED.into()
    };
    let socket = UdpSocket::bind(SocketAddr::new(wildcard, 0)).ok()?;
    // The port is never used; UDP "connecting" only picks a route.
    socket.connect(SocketAddr::new(speaker, 9)).ok()?;
    Some(socket.local_addr().ok()?.ip()).filter(|ip| !ip.is_unspecified())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_speaker_on_this_machine_is_reached_through_loopback() {
        assert_eq!(
            local_address_toward(Ipv4Addr::LOCALHOST.into()),
            Some(Ipv4Addr::LOCALHOST.into())
        );
    }
}

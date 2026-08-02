use alvr_common::{
    anyhow::{bail, Result},
    warn, ConnectionError, HandleTryAgain, ToAny, ALVR_NAME,
};
use alvr_sockets::{CONTROL_PORT, HANDSHAKE_PACKET_SIZE_BYTES, LOCAL_IP};
use flume::TryRecvError;
use mdns_sd::{Receiver, ServiceDaemon, ServiceEvent};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv6Addr, UdpSocket},
};

pub struct WelcomeSocket {
    buffer: [u8; HANDSHAKE_PACKET_SIZE_BYTES],
    broadcast_receiver: UdpSocket,
    mdns_receiver: Receiver<ServiceEvent>,
}

impl WelcomeSocket {
    pub fn new() -> Result<Self> {
        let socket = UdpSocket::bind((LOCAL_IP, CONTROL_PORT))?;
        // socket.set_read_timeout(Some(read_timeout))?;
        socket.set_nonblocking(true)?;

        let mdns_receiver = ServiceDaemon::new()?.browse(alvr_sockets::MDNS_SERVICE_TYPE)?;

        Ok(Self {
            buffer: [0; HANDSHAKE_PACKET_SIZE_BYTES],
            broadcast_receiver: socket,
            mdns_receiver,
        })
    }

    // Returns: client IP, client hostname
    pub fn recv_all(&mut self) -> Result<HashMap<String, IpAddr>> {
        let mut clients = HashMap::new();

        loop {
            match self
                .broadcast_receiver
                .recv_from(&mut self.buffer)
                .handle_try_again()
            {
                Ok((size, address)) => {
                    if size == HANDSHAKE_PACKET_SIZE_BYTES
                        && &self.buffer[..ALVR_NAME.len()] == ALVR_NAME.as_bytes()
                        && self.buffer[ALVR_NAME.len()..16].iter().all(|b| *b == 0)
                    {
                        let mut protocol_id_bytes = [0; 8];
                        protocol_id_bytes.copy_from_slice(&self.buffer[16..24]);
                        let received_protocol_id = u64::from_le_bytes(protocol_id_bytes);

                        if received_protocol_id != alvr_common::protocol_id_u64() {
                            warn!(
                                "Found incompatible client! Upgrade or downgrade\n{} {}, {} {}",
                                "Expected protocol ID",
                                alvr_common::protocol_id_u64(),
                                "Found",
                                received_protocol_id
                            );
                        }

                        let mut hostname_bytes = [0; 32];
                        hostname_bytes.copy_from_slice(&self.buffer[24..56]);
                        let hostname = std::str::from_utf8(&hostname_bytes)?
                            .trim_end_matches('\x00')
                            .to_owned();

                        insert_client_address(&mut clients, hostname, address.ip());
                    } else if &self.buffer[..16]
                        == b"\x00\x00\x00\x00\x04\x00\x00\x00\x00\x00\x00\x00ALVR"
                        || &self.buffer[..5] == b"\x01ALVR"
                    {
                        warn!("Found old client. Please upgrade")
                    } else {
                        // Unexpected packet.
                        // Note: no need to check for v12 and v13, not found in the wild anymore
                        warn!("Found unrelated packet during discovery")
                    }
                }
                Err(ConnectionError::TryAgain(_)) => break,
                Err(ConnectionError::Other(e)) => return Err(e),
            }
        }

        loop {
            match self.mdns_receiver.try_recv() {
                Ok(event) => {
                    if let ServiceEvent::ServiceResolved(info) = event {
                        let hostname = info
                            .get_property_val_str(alvr_sockets::MDNS_DEVICE_ID_KEY)
                            .unwrap_or_else(|| info.get_hostname());

                        // mDNS often lists link-local IPv6 first. Connecting to
                        // fe80::… without a scope id fails with EINVAL (os error 22)
                        // on Linux — prefer a global IPv4 when present.
                        let Some(address) = pick_connectable_address(info.get_addresses().iter().copied())
                        else {
                            warn!(
                                "Found client {hostname} via mDNS but no connectable address \
                                 (skipped link-local IPv6 without scope)"
                            );
                            continue;
                        };

                        let client_protocol = info
                            .get_property_val_str(alvr_sockets::MDNS_PROTOCOL_KEY)
                            .to_any()?;
                        let server_protocol = alvr_common::protocol_id();
                        let client_is_dev = client_protocol.contains("-dev");
                        let server_is_dev = server_protocol.contains("-dev");

                        if client_protocol != server_protocol {
                            let reason = if client_is_dev && server_is_dev {
                                "Please use matching nightly versions."
                            } else if client_is_dev {
                                "Please use nightly server or stable client."
                            } else if server_is_dev {
                                "Please use stable server or nightly client."
                            } else {
                                "Please use matching stable versions."
                            };
                            let protocols = format!(
                                "Protocols: server={server_protocol}, client={client_protocol}"
                            );
                            warn!("Found incompatible client {hostname}! {reason}\n{protocols}");
                        }

                        insert_client_address(&mut clients, hostname.into(), address);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(e) => bail!(e),
            }
        }

        Ok(clients)
    }
}

/// Prefer global IPv4, then global IPv6. Skip unspecified/loopback and
/// IPv6 link-local (needs a scope id that mDNS does not give us).
fn pick_connectable_address(addrs: impl IntoIterator<Item = IpAddr>) -> Option<IpAddr> {
    let mut best_v4 = None;
    let mut best_v6 = None;

    for addr in addrs {
        match addr {
            IpAddr::V4(ip) if !ip.is_unspecified() && !ip.is_loopback() && !ip.is_broadcast() => {
                best_v4.get_or_insert(addr);
            }
            IpAddr::V6(ip)
                if !ip.is_unspecified()
                    && !ip.is_loopback()
                    && !is_unicast_link_local_v6(ip)
                    && !ip.is_multicast() =>
            {
                best_v6.get_or_insert(addr);
            }
            _ => {}
        }
    }

    best_v4.or(best_v6)
}

fn is_unicast_link_local_v6(ip: Ipv6Addr) -> bool {
    // fe80::/10
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

/// Keep the better of two candidates for the same hostname (IPv4 wins).
fn insert_client_address(clients: &mut HashMap<String, IpAddr>, hostname: String, address: IpAddr) {
    match clients.get(&hostname) {
        Some(existing) if existing.is_ipv4() && address.is_ipv6() => {
            // Keep IPv4
        }
        _ => {
            clients.insert(hostname, address);
        }
    }
}

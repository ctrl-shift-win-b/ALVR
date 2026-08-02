use crate::LOCAL_IP;

use super::{SocketReader, SocketWriter};
use alvr_common::{
    anyhow::Result, con_bail, info, warn, ConResult, HandleTryAgain, ToCon,
};
use alvr_session::{DscpTos, SocketBufferSize};
use std::{
    io::Read,
    io::Write,
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
    thread,
    time::Duration,
};

pub fn bind(
    timeout: Duration,
    port: u16,
    dscp: Option<DscpTos>,
    send_buffer_bytes: SocketBufferSize,
    recv_buffer_bytes: SocketBufferSize,
) -> Result<TcpListener> {
    let socket = TcpListener::bind((LOCAL_IP, port))?.into();

    crate::set_socket_buffers(&socket, send_buffer_bytes, recv_buffer_bytes).ok();

    crate::set_dscp(&socket, dscp);

    socket.set_read_timeout(Some(timeout))?;

    Ok(socket.into())
}

pub fn accept_from_server(
    listener: &TcpListener,
    server_ip: Option<IpAddr>,
    timeout: Duration,
) -> ConResult<(TcpStream, TcpStream)> {
    // Uses timeout set during bind()
    let (socket, server_address) = listener.accept().handle_try_again()?;

    if let Some(ip) = server_ip {
        if server_address.ip() != ip {
            con_bail!(
                "Connected to wrong client: Expected: {ip}, Found {}",
                server_address.ip()
            );
        }
    }

    socket.set_read_timeout(Some(timeout)).to_con()?;
    socket.set_nodelay(true).to_con()?;

    Ok((socket.try_clone().to_con()?, socket))
}

pub fn connect_to_client(
    timeout: Duration,
    client_ips: &[IpAddr],
    port: u16,
    send_buffer_bytes: SocketBufferSize,
    recv_buffer_bytes: SocketBufferSize,
) -> ConResult<(TcpStream, TcpStream)> {
    if client_ips.is_empty() {
        con_bail!("No client IPs to connect to");
    }

    // Prefer IPv4: mDNS often yields link-local IPv6 that fails with EINVAL (22)
    // when used without a scope id. Also map IPv4-mapped IPv6 to real IPv4.
    let mut ordered: Vec<IpAddr> = client_ips
        .iter()
        .map(|ip| match ip {
            IpAddr::V6(v6) => v6
                .to_ipv4_mapped()
                .map(IpAddr::V4)
                .unwrap_or(*ip),
            other => *other,
        })
        .collect();
    ordered.sort_by_key(|ip| if ip.is_ipv4() { 0u8 } else { 1u8 });
    ordered.dedup();

    // Client control listen is intermittent; give each IP a longer window.
    let split_timeout = (timeout / ordered.len() as u32).max(Duration::from_millis(500));

    let mut last_err: Option<std::io::Error> = None;
    let mut socket_tcp: Option<TcpStream> = None;
    for ip in &ordered {
        // Skip unusable link-local IPv6 (no interface scope from discovery).
        if let IpAddr::V6(v6) = ip {
            if (v6.segments()[0] & 0xffc0) == 0xfe80 {
                warn!(
                    "Skipping link-local IPv6 {ip} for control connect (would cause EINVAL)"
                );
                continue;
            }
        }

        // A few quick retries: AVP opens :9943 only while advertising.
        for attempt in 0..3u32 {
            match TcpStream::connect_timeout(&SocketAddr::new(*ip, port), split_timeout) {
                Ok(s) => {
                    info!("Control TCP connected to {ip}:{port} (attempt {})", attempt + 1);
                    socket_tcp = Some(s);
                    break;
                }
                Err(e) => {
                    warn!(
                        "Control connect to {ip}:{port} attempt {} failed: {e} (os={:?})",
                        attempt + 1,
                        e.raw_os_error()
                    );
                    last_err = Some(e);
                    thread::sleep(Duration::from_millis(150));
                }
            }
        }
        if socket_tcp.is_some() {
            break;
        }
    }

    let socket = match socket_tcp {
        Some(s) => s.into(),
        None => {
            return Err(match last_err {
                Some(e) => std::result::Result::<TcpStream, _>::Err(e)
                    .handle_try_again()
                    .unwrap_err(),
                None => alvr_common::ConnectionError::Other(alvr_common::anyhow::anyhow!(
                    "No client IPs could be connected"
                )),
            });
        }
    };

    crate::set_socket_buffers(&socket, send_buffer_bytes, recv_buffer_bytes).ok();
    socket.set_read_timeout(Some(timeout)).to_con()?;

    let socket = TcpStream::from(socket);

    socket.set_nodelay(true).to_con()?;

    Ok((socket.try_clone().to_con()?, socket))
}

impl SocketWriter for TcpStream {
    fn send(&mut self, buffer: &[u8]) -> Result<()> {
        self.write_all(buffer)?;

        Ok(())
    }
}

impl SocketReader for TcpStream {
    fn recv(&mut self, buffer: &mut [u8]) -> ConResult<usize> {
        Read::read(self, buffer).handle_try_again()
    }

    fn peek(&self, buffer: &mut [u8]) -> ConResult<usize> {
        TcpStream::peek(self, buffer).handle_try_again()
    }
}

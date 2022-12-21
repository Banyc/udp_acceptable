use std::{io, os::fd::AsRawFd};

use nix::sys::socket::{
    setsockopt,
    sockopt::{Ipv4PacketInfo, Ipv6RecvPacketInfo},
};

use crate::{
    conn::UdpConn,
    early_pkt::channel::{EarlyPktSend, EarlyPktSendRes},
    recv::recv_from_to,
};

pub struct UdpListener {
    socket: socket2::Socket,
    early_pkt_sender: EarlyPktSend,
}

impl UdpListener {
    pub fn new(socket: socket2::Socket) -> io::Result<Self> {
        socket.set_reuse_address(true)?;
        socket.set_reuse_port(true)?;
        match socket
            .local_addr()?
            .as_socket()
            .ok_or(io::Error::new(
                io::ErrorKind::InvalidInput,
                "socket address is not a socket address",
            ))?
            .ip()
        {
            std::net::IpAddr::V4(_) => {
                setsockopt(socket.as_raw_fd(), Ipv4PacketInfo, &true)?;
            }
            std::net::IpAddr::V6(_) => {
                setsockopt(socket.as_raw_fd(), Ipv6RecvPacketInfo, &true)?;
            }
        }
        Ok(Self {
            socket,
            early_pkt_sender: EarlyPktSend::new(),
        })
    }

    /// <https://blog.cloudflare.com/everything-you-ever-wanted-to-know-about-udp-sockets-but-were-afraid-to-ask-part-1/>
    pub fn accept(&mut self, rx_buf: &mut [u8]) -> io::Result<(Option<UdpConn>, usize)> {
        let local_port = self.local_port()?;
        let (four_tuple, len) = recv_from_to(self.socket.as_raw_fd(), rx_buf, local_port)?;

        let buf = Vec::from(&rx_buf[..len]);

        // Send early packet to the existing connection.
        let res = self.early_pkt_sender.send(&four_tuple, buf);
        let buf = match res {
            EarlyPktSendRes::Ok => return Ok((None, len)),
            EarlyPktSendRes::Full(_) => return Ok((None, len)),
            EarlyPktSendRes::NotExist(buf) => buf,
        };

        // Create a new connection.
        let recv = self.early_pkt_sender.insert(four_tuple);
        let socket = socket2::Socket::new(
            match four_tuple.local_addr.ip() {
                std::net::IpAddr::V4(_) => socket2::Domain::IPV4,
                std::net::IpAddr::V6(_) => socket2::Domain::IPV6,
            },
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        socket.set_reuse_address(true)?;
        socket.set_reuse_port(true)?;
        socket.bind(&four_tuple.local_addr.into())?;
        socket.connect(&four_tuple.remote_addr.into())?;
        let conn = UdpConn::new(socket, four_tuple, recv);

        // Send early packet to the new connection.
        let res = self.early_pkt_sender.send(&conn.four_tuple(), buf);
        match res {
            EarlyPktSendRes::Ok => {}
            EarlyPktSendRes::Full(_) => {}
            EarlyPktSendRes::NotExist(_) => unreachable!(),
        }

        Ok((Some(conn), len))
    }

    pub fn socket(&self) -> &socket2::Socket {
        &self.socket
    }

    pub fn socket_mut(&mut self) -> &mut socket2::Socket {
        &mut self.socket
    }

    fn local_port(&self) -> io::Result<u16> {
        let port = self
            .socket
            .local_addr()?
            .as_socket()
            .ok_or(io::Error::new(
                io::ErrorKind::InvalidInput,
                "socket address is not a socket address",
            ))?
            .port();
        Ok(port)
    }
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

    #[test]
    #[serial]
    fn test_listen_ipv4_wildcard() {
        setup();
        let listen_port = 12345;
        let listen_addr_concrete = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), listen_port);
        let listen_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), listen_port);
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        socket.bind(&listen_addr.into()).unwrap();
        let mut listener = UdpListener::new(socket).unwrap();

        let send_port = 54321;
        let send_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), send_port);
        let send_socket = UdpSocket::bind(send_addr).unwrap();

        let send_buf = b"hello world";
        let send_len = send_socket.send_to(send_buf, listen_addr_concrete).unwrap();
        assert_eq!(send_len, send_buf.len());

        let mut recv_buf = [0u8; 1024];
        let (conn, recv_len) = listener.accept(&mut recv_buf).unwrap();
        assert_eq!(recv_len, send_len);
        assert_eq!(&recv_buf[..recv_len], send_buf);
        assert!(conn.is_some());
    }

    #[test]
    #[serial]
    fn test_listen_ipv4_specific() {
        setup();
        let listen_port = 12345;
        let listen_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), listen_port);
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        socket.bind(&listen_addr.into()).unwrap();
        let mut listener = UdpListener::new(socket).unwrap();

        let send_port = 54321;
        let send_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), send_port);
        let send_socket = UdpSocket::bind(send_addr).unwrap();

        let send_buf = b"hello world";
        let send_len = send_socket.send_to(send_buf, listen_addr).unwrap();
        assert_eq!(send_len, send_buf.len());

        let mut recv_buf = [0u8; 1024];
        let (conn, recv_len) = listener.accept(&mut recv_buf).unwrap();
        assert_eq!(recv_len, send_len);
        assert_eq!(&recv_buf[..recv_len], send_buf);
        assert!(conn.is_some());
    }

    #[test]
    #[serial]
    fn test_listen_ipv6_wildcard() {
        setup();
        let listen_port = 12345;
        let listen_addr_concrete = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), listen_port);
        let listen_addr = SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), listen_port);
        let socket = socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        socket.bind(&listen_addr.into()).unwrap();
        let mut listener = UdpListener::new(socket).unwrap();

        let send_port = 54321;
        let send_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), send_port);
        let send_socket = UdpSocket::bind(send_addr).unwrap();

        let send_buf = b"hello world";
        let send_len = send_socket.send_to(send_buf, listen_addr_concrete).unwrap();
        assert_eq!(send_len, send_buf.len());

        let mut recv_buf = [0u8; 1024];
        let (conn, recv_len) = listener.accept(&mut recv_buf).unwrap();
        assert_eq!(recv_len, send_len);
        assert_eq!(&recv_buf[..recv_len], send_buf);
        assert!(conn.is_some());
    }

    #[test]
    #[serial]
    fn test_listen_ipv6_specific() {
        setup();
        let listen_port = 12345;
        let listen_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), listen_port);
        let socket = socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        socket.bind(&listen_addr.into()).unwrap();
        let mut listener = UdpListener::new(socket).unwrap();

        let send_port = 54321;
        let send_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), send_port);
        let send_socket = UdpSocket::bind(send_addr).unwrap();

        let send_buf = b"hello world";
        let send_len = send_socket.send_to(send_buf, listen_addr).unwrap();
        assert_eq!(send_len, send_buf.len());

        let mut recv_buf = [0u8; 1024];
        let (conn, recv_len) = listener.accept(&mut recv_buf).unwrap();
        assert_eq!(recv_len, send_len);
        assert_eq!(&recv_buf[..recv_len], send_buf);
        assert!(conn.is_some());
    }

    #[test]
    #[serial]
    fn test_listen_ipv4_specific_many_clients() {
        setup();
        let listen_port = 12345;
        let listen_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), listen_port);
        let socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        socket.bind(&listen_addr.into()).unwrap();
        let mut listener = UdpListener::new(socket).unwrap();

        let send_port_start = 54321;
        let mut conns = Vec::new();
        for i in 0..100 {
            let send_port = send_port_start + i;
            let send_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), send_port);
            let send_socket = UdpSocket::bind(send_addr).unwrap();

            let send_buf = b"hello world";
            let send_len = send_socket.send_to(send_buf, listen_addr).unwrap();
            assert_eq!(send_len, send_buf.len());

            let mut recv_buf = [0u8; 1024];
            let (conn, recv_len) = listener.accept(&mut recv_buf).unwrap();
            assert_eq!(recv_len, send_len);
            assert_eq!(&recv_buf[..recv_len], send_buf);
            assert!(conn.is_some());

            conns.push(conn.unwrap());
        }
    }

    #[test]
    #[serial]
    fn test_listen_ipv6_specific_many_clients() {
        setup();
        let listen_port = 12345;
        let listen_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), listen_port);
        let socket = socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )
        .unwrap();
        socket.bind(&listen_addr.into()).unwrap();
        let mut listener = UdpListener::new(socket).unwrap();

        let send_port_start = 54321;
        let mut conns = Vec::new();
        for i in 0..100 {
            let send_port = send_port_start + i;
            let send_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), send_port);
            let send_socket = UdpSocket::bind(send_addr).unwrap();

            let send_buf = b"hello world";
            let send_len = send_socket.send_to(send_buf, listen_addr).unwrap();
            assert_eq!(send_len, send_buf.len());

            let mut recv_buf = [0u8; 1024];
            let (conn, recv_len) = listener.accept(&mut recv_buf).unwrap();
            assert_eq!(recv_len, send_len);
            assert_eq!(&recv_buf[..recv_len], send_buf);
            assert!(conn.is_some());

            conns.push(conn.unwrap());
        }
    }

    fn setup() {
        // wait for the OS to release the file descriptors
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

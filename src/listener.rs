use std::{
    borrow::Cow,
    collections::HashSet,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    os::fd::AsRawFd,
};

use futures::channel::mpsc;
use nix::sys::socket::{
    setsockopt,
    sockopt::{Ipv4PacketInfo, Ipv6RecvPacketInfo},
};

use crate::{
    channel::{ListenerChan, SendRes},
    conn::UdpConn,
    recv::{recv_from_to, FourTuple},
};

pub struct UdpListener {
    socket: socket2::Socket,
    chan: ListenerChan,
    local_ip_filter: IpFilter,
    non_blocking: bool,
}
impl UdpListener {
    pub fn bind(
        port: u16,
        local_ip_filter: IpFilterConfig,
        non_blocking: bool,
    ) -> io::Result<Self> {
        let socket = socket2::Socket::new(
            match local_ip_filter {
                IpFilterConfig::V4(_) => socket2::Domain::IPV4,
                IpFilterConfig::V6(_) => socket2::Domain::IPV6,
            },
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        let listen_addr = match local_ip_filter {
            IpFilterConfig::V4(_) => SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port),
            IpFilterConfig::V6(_) => SocketAddr::new(Ipv6Addr::UNSPECIFIED.into(), port),
        };
        socket.set_nonblocking(non_blocking)?;
        socket.set_reuse_address(true)?;
        match local_ip_filter {
            IpFilterConfig::V4(_) => {
                setsockopt(socket.as_raw_fd(), Ipv4PacketInfo, &true)?;
            }
            IpFilterConfig::V6(_) => {
                setsockopt(socket.as_raw_fd(), Ipv6RecvPacketInfo, &true)?;
            }
        }
        socket.bind(&listen_addr.into())?;
        Ok(Self {
            socket,
            chan: ListenerChan::new(),
            local_ip_filter: local_ip_filter.build(),
            non_blocking,
        })
    }

    /// <https://blog.cloudflare.com/everything-you-ever-wanted-to-know-about-udp-sockets-but-were-afraid-to-ask-part-1/>
    pub fn accept(&self, rx_buf: &mut [u8]) -> io::Result<(AcceptRes, FourTuple, usize)> {
        let local_port = self.local_port()?;
        let (four_tuple, len) = recv_from_to(self.socket.as_raw_fd(), rx_buf, local_port)?;

        let conn = self.accept_raw(&four_tuple, Cow::from(&rx_buf[..len]))?;

        Ok((conn, four_tuple, len))
    }

    pub fn accept_owned(&self, mut rx_buf: Vec<u8>) -> io::Result<(AcceptRes, FourTuple, usize)> {
        let local_port = self.local_port()?;
        let (four_tuple, len) = recv_from_to(self.socket.as_raw_fd(), &mut rx_buf, local_port)?;

        rx_buf.truncate(len);

        let conn = self.accept_raw(&four_tuple, Cow::from(rx_buf))?;

        Ok((conn, four_tuple, len))
    }

    pub fn recv_listener_pkt(&self) -> &mpsc::Receiver<(FourTuple, Vec<u8>)> {
        self.chan.recv_listener_pkt()
    }

    pub fn recv_listener_pkt_mut(&mut self) -> &mut mpsc::Receiver<(FourTuple, Vec<u8>)> {
        self.chan.recv_listener_pkt_mut()
    }

    /// `accept` but without `recvmsg`
    ///
    /// This is useful when a connection received a packet that is meant for this listener.
    pub fn accept_raw(&self, four_tuple: &FourTuple, rx_buf: Cow<[u8]>) -> io::Result<AcceptRes> {
        if !self.local_ip_filter.pass(&four_tuple.local_addr.ip()) {
            return Ok(AcceptRes::Filtered);
        }

        let buf = rx_buf.into_owned();

        // Send early packet to the existing connection.
        let res = self.chan.send_early_pkt(&four_tuple, buf);
        let buf = match res {
            SendRes::Ok => return Ok(AcceptRes::ConnAlreadyExists),
            SendRes::Full(_) => return Ok(AcceptRes::ConnAlreadyExists),
            SendRes::NotExist(buf) => buf,
        };

        // Create a new connection.
        let conn_chan = self.chan.create_early_pkt_chan(four_tuple.clone());
        let socket = socket2::Socket::new(
            match four_tuple.local_addr.ip() {
                std::net::IpAddr::V4(_) => socket2::Domain::IPV4,
                std::net::IpAddr::V6(_) => socket2::Domain::IPV6,
            },
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        socket.set_nonblocking(self.non_blocking)?;
        socket.set_reuse_address(true)?;
        socket.bind(&four_tuple.local_addr.into())?;
        socket.connect(&four_tuple.remote_addr.into())?;
        let conn = UdpConn::new(socket, four_tuple.clone(), conn_chan);

        // Send early packet to the new connection.
        let res = self.chan.send_early_pkt(&conn.four_tuple(), buf);
        match res {
            SendRes::Ok => {}
            SendRes::Full(_) => {}
            SendRes::NotExist(_) => unreachable!(),
        }

        Ok(AcceptRes::Ok(conn))
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

pub enum IpFilterConfig {
    V4(Option<HashSet<Ipv4Addr>>),
    V6(Option<HashSet<Ipv6Addr>>),
}
impl IpFilterConfig {
    fn build(self) -> IpFilter {
        match self {
            IpFilterConfig::V4(filter) => match filter {
                Some(filter) => IpFilter::V4(filter),
                None => IpFilter::AlwaysPass,
            },
            IpFilterConfig::V6(filter) => match filter {
                Some(filter) => IpFilter::V6(filter),
                None => IpFilter::AlwaysPass,
            },
        }
    }
}

enum IpFilter {
    V4(HashSet<Ipv4Addr>),
    V6(HashSet<Ipv6Addr>),
    AlwaysPass,
}
impl IpFilter {
    pub fn pass(&self, addr: &IpAddr) -> bool {
        match self {
            IpFilter::V4(filter) => match addr {
                IpAddr::V4(addr) => filter.contains(addr),
                IpAddr::V6(_) => false,
            },
            IpFilter::V6(filter) => match addr {
                IpAddr::V4(_) => false,
                IpAddr::V6(addr) => filter.contains(addr),
            },
            IpFilter::AlwaysPass => true,
        }
    }
}

pub enum AcceptRes {
    Ok(UdpConn),
    ConnAlreadyExists,
    Filtered,
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
        let listen_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), listen_port);
        let local_ip_filter = IpFilterConfig::V4(None);

        let listener = UdpListener::bind(listen_port, local_ip_filter, false).unwrap();

        let send_port = 54321;
        let send_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), send_port);
        let send_socket = UdpSocket::bind(send_addr).unwrap();

        let send_buf = b"hello world";
        let send_len = send_socket.send_to(send_buf, listen_addr).unwrap();
        assert_eq!(send_len, send_buf.len());

        let mut recv_buf = [0u8; 1024];
        let (res, _, recv_len) = listener.accept(&mut recv_buf).unwrap();
        assert_eq!(recv_len, send_len);
        assert_eq!(&recv_buf[..recv_len], send_buf);
        let AcceptRes::Ok(conn) = res else {
            panic!();
        };
        assert_eq!(conn.four_tuple().local_addr, listen_addr);
    }

    #[test]
    #[serial]
    fn test_listen_ipv4_specific() {
        setup();
        let listen_port = 12345;
        let listen_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), listen_port);
        let local_ip_filter =
            IpFilterConfig::V4(Some([Ipv4Addr::LOCALHOST].iter().cloned().collect()));

        let listener = UdpListener::bind(listen_port, local_ip_filter, false).unwrap();

        let send_port = 54321;
        let send_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), send_port);
        let send_socket = UdpSocket::bind(send_addr).unwrap();

        let send_buf = b"hello world";
        let send_len = send_socket.send_to(send_buf, listen_addr).unwrap();
        assert_eq!(send_len, send_buf.len());

        let mut recv_buf = [0u8; 1024];
        let (res, _, recv_len) = listener.accept(&mut recv_buf).unwrap();
        assert_eq!(recv_len, send_len);
        assert_eq!(&recv_buf[..recv_len], send_buf);
        let AcceptRes::Ok(conn) = res else {
            panic!();
        };
        assert_eq!(conn.four_tuple().local_addr, listen_addr);
    }

    #[test]
    #[serial]
    fn test_listen_ipv6_wildcard() {
        setup();
        let listen_port = 12345;
        let listen_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), listen_port);
        let local_ip_filter = IpFilterConfig::V6(None);

        let listener = UdpListener::bind(listen_port, local_ip_filter, false).unwrap();

        let send_port = 54321;
        let send_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), send_port);
        let send_socket = UdpSocket::bind(send_addr).unwrap();

        let send_buf = b"hello world";
        let send_len = send_socket.send_to(send_buf, listen_addr).unwrap();
        assert_eq!(send_len, send_buf.len());

        let mut recv_buf = [0u8; 1024];
        let (res, _, recv_len) = listener.accept(&mut recv_buf).unwrap();
        assert_eq!(recv_len, send_len);
        assert_eq!(&recv_buf[..recv_len], send_buf);
        let AcceptRes::Ok(conn) = res else {
            panic!();
        };
        assert_eq!(conn.four_tuple().local_addr, listen_addr);
    }

    #[test]
    #[serial]
    fn test_listen_ipv6_specific() {
        setup();
        let listen_port = 12345;
        let listen_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), listen_port);
        let local_ip_filter =
            IpFilterConfig::V6(Some([Ipv6Addr::LOCALHOST].iter().cloned().collect()));

        let listener = UdpListener::bind(listen_port, local_ip_filter, false).unwrap();

        let send_port = 54321;
        let send_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), send_port);
        let send_socket = UdpSocket::bind(send_addr).unwrap();

        let send_buf = b"hello world";
        let send_len = send_socket.send_to(send_buf, listen_addr).unwrap();
        assert_eq!(send_len, send_buf.len());

        let mut recv_buf = [0u8; 1024];
        let (res, _, recv_len) = listener.accept(&mut recv_buf).unwrap();
        assert_eq!(recv_len, send_len);
        assert_eq!(&recv_buf[..recv_len], send_buf);
        let AcceptRes::Ok(conn) = res else {
            panic!();
        };
        assert_eq!(conn.four_tuple().local_addr, listen_addr);
    }

    #[test]
    #[serial]
    fn test_listen_ipv4_specific_many_clients() {
        setup();
        let listen_port = 12345;
        let listen_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), listen_port);
        let local_ip_filter =
            IpFilterConfig::V4(Some([Ipv4Addr::LOCALHOST].iter().cloned().collect()));

        let listener = UdpListener::bind(listen_port, local_ip_filter, false).unwrap();

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
            let (res, _, recv_len) = listener.accept(&mut recv_buf).unwrap();
            assert_eq!(recv_len, send_len);
            assert_eq!(&recv_buf[..recv_len], send_buf);
            let AcceptRes::Ok(conn) = res else {
                panic!();
            };
            assert_eq!(conn.four_tuple().local_addr, listen_addr);

            conns.push(conn);
        }
    }

    #[test]
    #[serial]
    fn test_listen_ipv6_specific_many_clients() {
        setup();
        let listen_port = 12345;
        let listen_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), listen_port);
        let local_ip_filter =
            IpFilterConfig::V6(Some([Ipv6Addr::LOCALHOST].iter().cloned().collect()));

        let listener = UdpListener::bind(listen_port, local_ip_filter, false).unwrap();

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
            let (res, _, recv_len) = listener.accept(&mut recv_buf).unwrap();
            assert_eq!(recv_len, send_len);
            assert_eq!(&recv_buf[..recv_len], send_buf);
            let AcceptRes::Ok(conn) = res else {
                panic!();
            };
            assert_eq!(conn.four_tuple().local_addr, listen_addr);

            conns.push(conn);
        }
    }

    fn setup() {
        // wait for the OS to release the file descriptors
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

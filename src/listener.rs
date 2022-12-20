use std::{io, os::unix::prelude::AsRawFd};

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
                setsockopt(socket.as_raw_fd(), Ipv4PacketInfo, &true).unwrap();
            }
            std::net::IpAddr::V6(_) => {
                setsockopt(socket.as_raw_fd(), Ipv6RecvPacketInfo, &true).unwrap();
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

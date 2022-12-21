use std::{
    io::{self, IoSliceMut},
    net::{Ipv4Addr, SocketAddr},
    os::fd::RawFd,
};

use nix::{
    cmsg_space, libc,
    sys::socket::{recvmsg, ControlMessageOwned, MsgFlags, SockaddrStorage},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FourTuple {
    pub local_addr: SocketAddr,
    pub remote_addr: SocketAddr,
}

/// <https://blog.cloudflare.com/everything-you-ever-wanted-to-know-about-udp-sockets-but-were-afraid-to-ask-part-1/>
pub fn recv_from_to(
    fd: RawFd,
    rx_buf: &mut [u8],
    listen_port: u16,
) -> io::Result<(FourTuple, usize)> {
    // struct iovec { /* Scatter/gather array items */
    //     void  *iov_base;              /* Starting address */
    //     size_t iov_len;               /* Number of bytes to transfer */ };
    let mut iov = [IoSliceMut::new(rx_buf)];

    // struct msghdr {
    //     void         *msg_name;       /* Optional address */
    //     socklen_t     msg_namelen;    /* Size of address */
    //     struct iovec *msg_iov;        /* Scatter/gather array */
    //     size_t        msg_iovlen;     /* # elements in msg_iov */
    //     void         *msg_control;    /* Ancillary data, see below */
    //     size_t        msg_controllen; /* Ancillary data buffer len */
    //     int           msg_flags;      /* Flags on received message */ };

    // sizeof(in6_pktinfo) > sizeof(in_pktinfo)
    let mut cmsg_space = cmsg_space!(libc::in6_pktinfo);
    let msg = recvmsg::<SockaddrStorage>(fd, &mut iov, Some(&mut cmsg_space), MsgFlags::empty())?;

    // struct cmsghdr {
    //     size_t cmsg_len;    /* Data byte count, including header
    //                            (type is socklen_t in POSIX) */
    //     int    cmsg_level;  /* Originating protocol */
    //     int    cmsg_type;   /* Protocol-specific type */ /* followed by
    //     unsigned char cmsg_data[]; */ };

    // Ancillary data should be accessed only by the macros defined in cmsg(3).

    // Get local address.
    let mut local_addr_ip = None;
    for cmsg in msg.cmsgs() {
        match cmsg {
            ControlMessageOwned::Ipv4PacketInfo(info) => {
                local_addr_ip = Some(in_addr_to_std(&info.ipi_addr).into());
            }
            ControlMessageOwned::Ipv6PacketInfo(info) => {
                local_addr_ip = Some(info.ipi6_addr.s6_addr.into());
            }
            _ => {}
        }
    }
    let local_addr_ip = local_addr_ip.ok_or(io::Error::new(
        io::ErrorKind::Other,
        "recvmsg did not return a local address",
    ))?;
    let local_addr = SocketAddr::new(local_addr_ip, listen_port);

    // Get remote address.
    let remote_addr = msg.address.ok_or(io::Error::new(
        io::ErrorKind::Other,
        "recvmsg did not return a remote address",
    ))?;
    // Convert to SocketAddr.
    let remote_addr = storage_to_std(remote_addr).ok_or(io::Error::new(
        io::ErrorKind::Other,
        "recvmsg returned an invalid remote address",
    ))?;

    let four_tuple = FourTuple {
        local_addr,
        remote_addr,
    };

    Ok((four_tuple, msg.bytes))
}

fn storage_to_std(ss: SockaddrStorage) -> Option<SocketAddr> {
    match ss.as_sockaddr_in() {
        Some(sin) => {
            return Some(sockaddr_in_to_std(sin.as_ref()));
        }
        None => (),
    };
    match ss.as_sockaddr_in6() {
        Some(sin6) => {
            return Some(sockaddr_in6_to_std(sin6.as_ref()));
        }
        None => (),
    };
    None
}

fn in_addr_to_std(ia: &libc::in_addr) -> Ipv4Addr {
    // Convert from big-endian to host byte order.
    let s_addr = u32::from_be(ia.s_addr);
    Ipv4Addr::from(s_addr)
}

fn sockaddr_in_to_std(sa: &libc::sockaddr_in) -> SocketAddr {
    let ip = in_addr_to_std(&sa.sin_addr);
    let port = u16::from_be(sa.sin_port);
    SocketAddr::new(ip.into(), port)
}

fn sockaddr_in6_to_std(sa: &libc::sockaddr_in6) -> SocketAddr {
    let ip = sa.sin6_addr.s6_addr;
    let port = u16::from_be(sa.sin6_port);
    SocketAddr::new(ip.into(), port)
}

#[cfg(test)]
mod tests {
    use nix::sys::socket::{
        setsockopt,
        sockopt::{Ipv4PacketInfo, Ipv6RecvPacketInfo},
    };

    use super::*;
    use std::{
        net::{Ipv6Addr, UdpSocket},
        os::fd::AsRawFd,
    };

    #[test]
    fn test_recv_from_to_ipv4() {
        let listen_port = 12345;
        let listen_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), listen_port);
        let listen_socket = UdpSocket::bind(listen_addr).unwrap();
        let listen_fd = listen_socket.as_raw_fd();
        setsockopt(listen_fd, Ipv4PacketInfo, &true).unwrap();

        let send_port = 54321;
        let send_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), send_port);
        let send_socket = UdpSocket::bind(send_addr).unwrap();

        let send_buf = b"hello world";
        let send_len = send_socket.send_to(send_buf, listen_addr).unwrap();
        assert_eq!(send_len, send_buf.len());

        let mut rx_buf = [0u8; 1024];
        let (four_tuple, recv_len) = recv_from_to(listen_fd, &mut rx_buf, listen_port).unwrap();
        assert_eq!(recv_len, send_len);
        assert_eq!(four_tuple.local_addr, listen_addr);
        assert_eq!(four_tuple.remote_addr, send_addr);
        assert_eq!(&rx_buf[..recv_len], send_buf);
    }

    #[test]
    fn test_recv_from_to_ipv6() {
        let listen_port = 12345;
        let listen_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), listen_port);
        let listen_socket = UdpSocket::bind(listen_addr).unwrap();
        let listen_fd = listen_socket.as_raw_fd();
        setsockopt(listen_fd, Ipv6RecvPacketInfo, &true).unwrap();

        let send_port = 54321;
        let send_addr = SocketAddr::new(Ipv6Addr::LOCALHOST.into(), send_port);
        let send_socket = UdpSocket::bind(send_addr).unwrap();

        let send_buf = b"hello world";
        let send_len = send_socket.send_to(send_buf, listen_addr).unwrap();
        assert_eq!(send_len, send_buf.len());

        let mut rx_buf = [0u8; 1024];
        let (four_tuple, recv_len) = recv_from_to(listen_fd, &mut rx_buf, listen_port).unwrap();
        assert_eq!(recv_len, send_len);
        assert_eq!(four_tuple.local_addr, listen_addr);
        assert_eq!(four_tuple.remote_addr, send_addr);
        assert_eq!(&rx_buf[..recv_len], send_buf);
    }
}

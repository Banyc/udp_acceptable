use std::{io, os::fd::AsRawFd};

use crate::{
    channel::ConnChan,
    recv::{recv_from_to, FourTuple},
};

pub struct UdpConn {
    socket: socket2::Socket,
    four_tuple: FourTuple,
    chan: ConnChan,
}

impl UdpConn {
    pub fn new(socket: socket2::Socket, four_tuple: FourTuple, chan: ConnChan) -> Self {
        Self {
            socket,
            four_tuple,
            chan,
        }
    }

    pub fn socket(&self) -> &socket2::Socket {
        &self.socket
    }

    pub fn socket_mut(&mut self) -> &mut socket2::Socket {
        &mut self.socket
    }

    /// Receive a packet from the socket, not from the early packet channel.
    ///
    /// Returns the number of bytes received.
    ///
    /// If the received packet is not meant for this connection, returns `RecvRes::ListenerPkt`.
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<(RecvRes, usize)> {
        let (four_tuple, len) = recv_from_to(
            self.socket.as_raw_fd(),
            buf,
            self.four_tuple.local_addr.port(),
        )?;
        if four_tuple != self.four_tuple {
            return Ok((RecvRes::ListenerPkt(four_tuple), len));
        }
        Ok((RecvRes::Ok, len))
    }

    /// Receiver of the early packet channel.
    pub fn recv_early_pkt(&self) -> &ConnChan {
        &self.chan
    }

    pub fn recv_early_pkt_mut(&mut self) -> &mut ConnChan {
        &mut self.chan
    }

    pub fn four_tuple(&self) -> &FourTuple {
        &self.four_tuple
    }
}

pub enum RecvRes {
    Ok,
    ListenerPkt(FourTuple),
}

use crate::{early_pkt::channel::EarlyPktRecv, recv::FourTuple};

pub struct UdpConn {
    socket: socket2::Socket,
    four_tuple: FourTuple,
    early_pkt_recv: EarlyPktRecv,
}

impl UdpConn {
    pub fn new(
        socket: socket2::Socket,
        four_tuple: FourTuple,
        early_pkt_recv: EarlyPktRecv,
    ) -> Self {
        Self {
            socket,
            four_tuple,
            early_pkt_recv,
        }
    }

    pub fn socket(&self) -> &socket2::Socket {
        &self.socket
    }

    pub fn socket_mut(&mut self) -> &mut socket2::Socket {
        &mut self.socket
    }

    pub fn early_pkt_recv(&self) -> &EarlyPktRecv {
        &self.early_pkt_recv
    }

    pub fn early_pkt_recv_mut(&mut self) -> &mut EarlyPktRecv {
        &mut self.early_pkt_recv
    }

    pub fn four_tuple(&self) -> &FourTuple {
        &self.four_tuple
    }
}

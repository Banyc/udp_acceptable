use std::sync::{Arc, RwLock, Weak};

use futures::channel::mpsc;

use crate::recv::FourTuple;

use super::map::EarlyPktMap;

pub struct ConnChan {
    early_pkt_map: Weak<RwLock<EarlyPktMap>>,
    early_pkt_key: FourTuple,
    early_pkt_recv: mpsc::Receiver<Vec<u8>>,
    listener_pkt_send: mpsc::Sender<(FourTuple, Vec<u8>)>,
}
impl ConnChan {
    pub fn remove(&self) {
        let Some(map) = self.early_pkt_map.upgrade() else {
            return;
        };
        map.write().unwrap().remove(&self.early_pkt_key);
    }

    pub fn recv_early_pkt(&self) -> &mpsc::Receiver<Vec<u8>> {
        &self.early_pkt_recv
    }

    pub fn send_listener_pkt(&mut self, four_tuple: FourTuple, buf: Vec<u8>) -> SendRes {
        match self.listener_pkt_send.try_send((four_tuple, buf)) {
            Ok(()) => SendRes::Ok,
            Err(e) => {
                if e.is_full() {
                    SendRes::Full(e.into_inner().1)
                } else if e.is_disconnected() {
                    SendRes::NotExist(e.into_inner().1)
                } else {
                    unreachable!()
                }
            }
        }
    }
}
impl Drop for ConnChan {
    fn drop(&mut self) {
        self.remove();
    }
}

pub struct ListenerChan {
    early_pkt_map: Arc<RwLock<EarlyPktMap>>,
    listener_pkt_send: mpsc::Sender<(FourTuple, Vec<u8>)>,
    listener_pkt_recv: mpsc::Receiver<(FourTuple, Vec<u8>)>,
}
impl ListenerChan {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel(1);
        Self {
            early_pkt_map: Arc::new(RwLock::new(EarlyPktMap::new())),
            listener_pkt_send: sender,
            listener_pkt_recv: receiver,
        }
    }

    pub fn create_early_pkt_chan(&self, four_tuple: FourTuple) -> ConnChan {
        let (sender, receiver) = mpsc::channel(1);
        self.early_pkt_map
            .write()
            .unwrap()
            .insert(four_tuple, sender);
        ConnChan {
            early_pkt_map: Arc::downgrade(&self.early_pkt_map),
            early_pkt_key: four_tuple,
            early_pkt_recv: receiver,
            listener_pkt_send: self.listener_pkt_send.clone(),
        }
    }

    pub fn send_early_pkt(&self, four_tuple: &FourTuple, buf: Vec<u8>) -> SendRes {
        let mut map = self.early_pkt_map.write().unwrap();
        let Some(sender) = map.get_mut(four_tuple) else {
            return SendRes::NotExist(buf);
        };
        match sender.try_send(buf) {
            Ok(_) => SendRes::Ok,
            Err(e) => {
                if e.is_full() {
                    SendRes::Full(e.into_inner())
                } else if e.is_disconnected() {
                    map.remove(four_tuple);
                    SendRes::NotExist(e.into_inner())
                } else {
                    unreachable!()
                }
            }
        }
    }

    pub fn recv_listener_pkt(&self) -> &mpsc::Receiver<(FourTuple, Vec<u8>)> {
        &self.listener_pkt_recv
    }

    pub fn recv_listener_pkt_mut(&mut self) -> &mut mpsc::Receiver<(FourTuple, Vec<u8>)> {
        &mut self.listener_pkt_recv
    }
}

pub enum SendRes {
    Ok,
    Full(Vec<u8>),
    NotExist(Vec<u8>),
}

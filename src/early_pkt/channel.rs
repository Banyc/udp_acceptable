use std::sync::{Arc, RwLock, Weak};

use futures::channel::mpsc;

use crate::recv::FourTuple;

use super::map::EarlyPktMap;

pub struct EarlyPktRecv {
    map: Weak<RwLock<EarlyPktMap>>,
    key: FourTuple,
    recv: mpsc::Receiver<Vec<u8>>,
}
impl EarlyPktRecv {
    pub fn remove(&self) {
        let Some(map) = self.map.upgrade() else {
            return;
        };
        map.write().unwrap().remove(&self.key);
    }

    pub fn recv(&self) -> &mpsc::Receiver<Vec<u8>> {
        &self.recv
    }
}
impl Drop for EarlyPktRecv {
    fn drop(&mut self) {
        self.remove();
    }
}

pub struct EarlyPktSend {
    map: Arc<RwLock<EarlyPktMap>>,
}
impl EarlyPktSend {
    pub fn new() -> Self {
        Self {
            map: Arc::new(RwLock::new(EarlyPktMap::new())),
        }
    }

    pub fn insert(&self, four_tuple: FourTuple) -> EarlyPktRecv {
        let sender = mpsc::channel(1).0;
        self.map.write().unwrap().insert(four_tuple, sender);
        EarlyPktRecv {
            map: Arc::downgrade(&self.map),
            key: four_tuple,
            recv: mpsc::channel(1).1,
        }
    }

    pub fn send(&self, four_tuple: &FourTuple, buf: Vec<u8>) -> EarlyPktSendRes {
        let mut map = self.map.write().unwrap();
        let Some(sender) = map.get_mut(four_tuple) else {
            return EarlyPktSendRes::NotExist(buf);
        };
        match sender.try_send(buf) {
            Ok(_) => EarlyPktSendRes::Ok,
            Err(e) => {
                if e.is_full() {
                    EarlyPktSendRes::Full(e.into_inner())
                } else if e.is_disconnected() {
                    map.remove(four_tuple);
                    EarlyPktSendRes::NotExist(e.into_inner())
                } else {
                    unreachable!()
                }
            }
        }
    }
}

pub enum EarlyPktSendRes {
    Ok,
    Full(Vec<u8>),
    NotExist(Vec<u8>),
}

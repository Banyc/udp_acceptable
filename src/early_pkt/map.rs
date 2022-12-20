use std::collections::HashMap;

use futures::channel::mpsc;

use crate::recv::FourTuple;

pub struct EarlyPktMap {
    map: HashMap<FourTuple, mpsc::Sender<Vec<u8>>>,
}
impl EarlyPktMap {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn insert(&mut self, four_tuple: FourTuple, sender: mpsc::Sender<Vec<u8>>) {
        self.map.insert(four_tuple, sender);
    }

    pub fn get_mut(&mut self, four_tuple: &FourTuple) -> Option<&mut mpsc::Sender<Vec<u8>>> {
        self.map.get_mut(four_tuple)
    }

    pub fn remove(&mut self, four_tuple: &FourTuple) {
        self.map.remove(four_tuple);
    }
}

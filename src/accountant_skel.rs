use accountant::Accountant;
use transaction::Transaction;
use signature::PublicKey;
use hash::Hash;
use entry::Entry;
use std::net::UdpSocket;
use bincode::{deserialize, serialize};
use result::Result;
use streamer;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::thread::{spawn, JoinHandle};
use std::default::Default;
use serde_json;

pub struct AccountantSkel {
    pub acc: Accountant,
    pub last_id: Hash,
    pub ledger: Vec<Entry>,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Request {
    Transaction(Transaction),
    GetBalance { key: PublicKey },
    GetEntries { last_id: Hash },
    GetId { is_last: bool },
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Response {
    Balance { key: PublicKey, val: Option<i64> },
    Entries { entries: Vec<Entry> },
    Id { id: Hash, is_last: bool },
}

impl AccountantSkel {
    pub fn new(acc: Accountant) -> Self {
        let last_id = acc.first_id;
        AccountantSkel {
            acc,
            last_id,
            ledger: vec![],
        }
    }

    pub fn sync(self: &mut Self) -> Hash {
        while let Ok(entry) = self.acc.historian.receiver.try_recv() {
            self.last_id = entry.id;
            println!("{}", serde_json::to_string(&entry).unwrap());
            self.ledger.push(entry);
        }
        self.last_id
    }

    pub fn process_request(self: &mut Self, msg: Request) -> Option<Response> {
        match msg {
            Request::Transaction(tr) => {
                if let Err(err) = self.acc.process_transaction(tr) {
                    eprintln!("Transaction error: {:?}", err);
                }
                None
            }
            Request::GetBalance { key } => {
                let val = self.acc.get_balance(&key);
                Some(Response::Balance { key, val })
            }
            Request::GetEntries { last_id } => {
                self.sync();
                let entries = self.ledger
                    .iter()
                    .skip_while(|x| x.id != last_id) // log(n) way to find Entry with id == last_id.
                    .skip(1) // Skip the entry with last_id.
                    .take(256) // TODO: Take while the serialized entries fit into a 64k UDP packet.
                    .cloned()
                    .collect();
                Some(Response::Entries { entries })
            }
            Request::GetId { is_last } => Some(Response::Id {
                id: if is_last {
                    self.sync()
                } else {
                    self.acc.first_id
                },
                is_last,
            }),
        }
    }
    fn process(
        &mut self,
        r_reader: &streamer::Receiver,
        s_sender: &streamer::Sender,
        recycler: &streamer::Recycler,
    ) -> Result<()> {
        let timer = Duration::new(1, 0);
        let msgs = r_reader.recv_timeout(timer)?;
        let msgs_ = msgs.clone();
        let msgs__ = msgs.clone();
        let rsps = streamer::allocate(recycler);
        let rsps_ = rsps.clone();
        let l = msgs__.read().unwrap().packets.len();
        rsps.write()
            .unwrap()
            .packets
            .resize(l, streamer::Packet::default());
        {
            let mut num = 0;
            let mut ursps = rsps.write().unwrap();
            for packet in &msgs.read().unwrap().packets {
                let sz = packet.size;
                let req = deserialize(&packet.data[0..sz])?;
                if let Some(resp) = self.process_request(req) {
                    let rsp = &mut ursps.packets[num];
                    let v = serialize(&resp)?;
                    let len = v.len();
                    rsp.data[0..len].copy_from_slice(&v);
                    rsp.size = len;
                    rsp.set_addr(&packet.get_addr());
                    num += 1;
                }
            }
            ursps.packets.resize(num, streamer::Packet::default());
        }
        s_sender.send(rsps_)?;
        streamer::recycle(recycler, msgs_);
        Ok(())
    }

    /// UDP Server that forwards messages to Accountant methods.
    pub fn serve(
        obj: Arc<Mutex<AccountantSkel>>,
        addr: &str,
        exit: Arc<AtomicBool>,
    ) -> Result<[Arc<JoinHandle<()>>; 3]> {
        let read = UdpSocket::bind(addr)?;
        // make sure we are on the same interface
        let mut local = read.local_addr()?;
        local.set_port(0);
        let write = UdpSocket::bind(local)?;

        let recycler = Arc::new(Mutex::new(Vec::new()));
        let (s_reader, r_reader) = channel();
        let t_receiver = streamer::receiver(read, exit.clone(), recycler.clone(), s_reader)?;

        let (s_sender, r_sender) = channel();
        let t_sender = streamer::sender(write, exit.clone(), recycler.clone(), r_sender);

        let t_server = spawn(move || {
            if let Ok(me) = Arc::try_unwrap(obj) {
                loop {
                    let e = me.lock().unwrap().process(&r_reader, &s_sender, &recycler);
                    if e.is_err() && exit.load(Ordering::Relaxed) {
                        break;
                    }
                }
            }
        });
        Ok([Arc::new(t_receiver), Arc::new(t_sender), Arc::new(t_server)])
    }
}

//! The `accountant` is a client of the `historian`. It uses the historian's
//! event log to record transactions. Its users can deposit funds and
//! transfer funds to other users.

use std::net::UdpSocket;
use std::io;
use bincode::{deserialize, serialize};
use transaction::Transaction;
use signature::{KeyPair, PublicKey, Signature};
use hash::Hash;
use entry::Entry;
use accountant_skel::{Request, Response};

pub struct AccountantStub {
    pub addr: String,
    pub socket: UdpSocket,
}

impl AccountantStub {
    pub fn new(addr: &str, socket: UdpSocket) -> Self {
        AccountantStub {
            addr: addr.to_string(),
            socket,
        }
    }

    pub fn transfer_signed(&self, tr: Transaction) -> io::Result<usize> {
        let req = Request::Transaction(tr);
        let data = serialize(&req).unwrap();
        self.socket.send_to(&data, &self.addr)
    }

    pub fn transfer(
        &self,
        n: i64,
        keypair: &KeyPair,
        to: PublicKey,
        last_id: &Hash,
    ) -> io::Result<Signature> {
        let tr = Transaction::new(keypair, to, n, *last_id);
        let sig = tr.sig;
        self.transfer_signed(tr).map(|_| sig)
    }

    pub fn get_balance(&self, pubkey: &PublicKey) -> io::Result<Option<i64>> {
        let req = Request::GetBalance { key: *pubkey };
        let data = serialize(&req).expect("serialize GetBalance");
        self.socket.send_to(&data, &self.addr)?;
        let mut buf = vec![0u8; 1024];
        self.socket.recv_from(&mut buf)?;
        let resp = deserialize(&buf).expect("deserialize balance");
        if let Response::Balance { key, val } = resp {
            assert_eq!(key, *pubkey);
            return Ok(val);
        }
        Ok(None)
    }

    fn get_id(&self, is_last: bool) -> io::Result<Hash> {
        let req = Request::GetId { is_last };
        let data = serialize(&req).expect("serialize GetId");
        self.socket.send_to(&data, &self.addr)?;
        let mut buf = vec![0u8; 1024];
        self.socket.recv_from(&mut buf)?;
        let resp = deserialize(&buf).expect("deserialize Id");
        if let Response::Id { id, .. } = resp {
            return Ok(id);
        }
        Ok(Default::default())
    }

    pub fn get_last_id(&self) -> io::Result<Hash> {
        self.get_id(true)
    }

    pub fn check_on_signature(
        &mut self,
        wait_sig: &Signature,
        last_id: &Hash,
    ) -> io::Result<(bool, Hash)> {
        let mut last_id = *last_id;
        let req = Request::GetEntries { last_id };
        let data = serialize(&req).unwrap();
        self.socket.send_to(&data, &self.addr).map(|_| ())?;

        let mut buf = vec![0u8; 65_535];
        self.socket.recv_from(&mut buf)?;
        let resp = deserialize(&buf).expect("deserialize signature");
        let mut found = false;
        if let Response::Entries { entries } = resp {
            for Entry { id, events, .. } in entries {
                last_id = id;
                if !found {
                    for event in events {
                        if let Some(sig) = event.get_signature() {
                            if sig == *wait_sig {
                                found = true;
                            }
                        }
                    }
                }
            }
        }

        Ok((found, last_id))
    }

    pub fn wait_on_signature(&mut self, wait_sig: &Signature, last_id: &Hash) -> io::Result<Hash> {
        let mut found = false;
        let mut last_id = *last_id;
        while !found {
            let ret = self.check_on_signature(wait_sig, &last_id)?;
            found = ret.0;
            last_id = ret.1;
        }
        Ok(last_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use accountant::Accountant;
    use accountant_skel::AccountantSkel;
    use std::thread::sleep;
    use std::time::Duration;
    use mint::Mint;
    use signature::{KeyPair, KeyPairUtil};
    use std::sync::{Arc, Mutex};
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn test_accountant_stub() {
        let addr = "127.0.0.1:9000";
        let send_addr = "127.0.0.1:9001";
        let alice = Mint::new(10_000);
        let acc = Accountant::new(&alice, Some(30));
        let bob_pubkey = KeyPair::new().pubkey();
        let exit = Arc::new(AtomicBool::new(false));
        let acc = Arc::new(Mutex::new(AccountantSkel::new(acc)));
        let threads = AccountantSkel::serve(acc, addr, exit.clone()).unwrap();
        sleep(Duration::from_millis(30));

        let socket = UdpSocket::bind(send_addr).unwrap();
        let mut acc = AccountantStub::new(addr, socket);
        let last_id = acc.get_last_id().unwrap();
        let sig = acc.transfer(500, &alice.keypair(), bob_pubkey, &last_id)
            .unwrap();
        acc.wait_on_signature(&sig, &last_id).unwrap();
        assert_eq!(acc.get_balance(&bob_pubkey).unwrap().unwrap(), 500);
        exit.store(true, Ordering::Relaxed);
        for t in threads {
            t.join().expect("join");
        }
    }
}

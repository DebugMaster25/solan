//! The `shred_fetch_stage` pulls shreds from UDP sockets and sends it to a channel.

use bv::BitVec;
use solana_ledger::bank_forks::BankForks;
use solana_ledger::blockstore::MAX_DATA_SHREDS_PER_SLOT;
use solana_ledger::shred::{
    OFFSET_OF_SHRED_INDEX, OFFSET_OF_SHRED_SLOT, SIZE_OF_SHRED_INDEX, SIZE_OF_SHRED_SLOT,
};
use solana_perf::cuda_runtime::PinnedVec;
use solana_perf::packet::{limited_deserialize, Packet, PacketsRecycler};
use solana_perf::recycler::Recycler;
use solana_sdk::clock::Slot;
use solana_streamer::streamer::{self, PacketReceiver, PacketSender};
use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::sync::RwLock;
use std::thread::{self, Builder, JoinHandle};
use std::time::Instant;

pub type ShredsReceived = HashMap<Slot, BitVec<u64>>;

pub struct ShredFetchStage {
    thread_hdls: Vec<JoinHandle<()>>,
}

impl ShredFetchStage {
    fn get_slot_index(p: &Packet, index_overrun: &mut usize) -> Option<(u64, u32)> {
        let index_start = OFFSET_OF_SHRED_INDEX;
        let index_end = index_start + SIZE_OF_SHRED_INDEX;
        let slot_start = OFFSET_OF_SHRED_SLOT;
        let slot_end = slot_start + SIZE_OF_SHRED_SLOT;

        if index_end <= p.meta.size {
            if let Ok(index) = limited_deserialize::<u32>(&p.data[index_start..index_end]) {
                if index < MAX_DATA_SHREDS_PER_SLOT as u32 && slot_end <= p.meta.size {
                    if let Ok(slot) = limited_deserialize::<Slot>(&p.data[slot_start..slot_end]) {
                        return Some((slot, index));
                    }
                }
            }
        } else {
            *index_overrun += 1;
        }
        None
    }

    fn process_packet<F>(
        p: &mut Packet,
        shreds_received: &mut ShredsReceived,
        index_overrun: &mut usize,
        last_root: Slot,
        last_slot: Slot,
        slots_per_epoch: u64,
        modify: &F,
    ) where
        F: Fn(&mut Packet),
    {
        p.meta.discard = true;
        if let Some((slot, index)) = Self::get_slot_index(p, index_overrun) {
            // Seems reasonable to limit shreds to 2 epochs away
            if slot > last_root && slot < (last_slot + 2 * slots_per_epoch) {
                // Shred filter
                let slot_received = shreds_received
                    .entry(slot)
                    .or_insert_with(|| BitVec::new_fill(false, MAX_DATA_SHREDS_PER_SLOT as u64));
                if !slot_received.get(index.into()) {
                    p.meta.discard = false;
                    modify(p);
                    slot_received.set(index.into(), true);
                }
            }
        }
    }

    // updates packets received on a channel and sends them on another channel
    fn modify_packets<F>(
        recvr: PacketReceiver,
        sendr: PacketSender,
        bank_forks: Option<Arc<RwLock<BankForks>>>,
        modify: F,
    ) where
        F: Fn(&mut Packet),
    {
        let mut shreds_received = ShredsReceived::default();
        let mut last_cleared = Instant::now();

        // In the case of bank_forks=None, setup to accept any slot range
        let mut last_root = 0;
        let mut last_slot = std::u64::MAX;
        let mut slots_per_epoch = 0;

        while let Some(mut p) = recvr.iter().next() {
            if last_cleared.elapsed().as_millis() > 200 {
                shreds_received.clear();
                last_cleared = Instant::now();
                if let Some(bank_forks) = bank_forks.as_ref() {
                    let bank_forks_r = bank_forks.read().unwrap();
                    last_root = bank_forks_r.root();
                    let working_bank = bank_forks_r.working_bank();
                    last_slot = working_bank.slot();
                    let root_bank = bank_forks_r.root_bank();
                    slots_per_epoch = root_bank.get_slots_in_epoch(root_bank.epoch());
                }
            }
            let mut index_overrun = 0;
            let mut shred_count = 0;
            p.packets.iter_mut().for_each(|mut packet| {
                shred_count += 1;
                Self::process_packet(
                    &mut packet,
                    &mut shreds_received,
                    &mut index_overrun,
                    last_root,
                    last_slot,
                    slots_per_epoch,
                    &modify,
                );
            });
            inc_new_counter_warn!("shred_fetch_stage-shred_index_overrun", index_overrun);
            inc_new_counter_info!("shred_fetch_stage-shred_count", shred_count);
            if sendr.send(p).is_err() {
                break;
            }
        }
    }

    fn packet_modifier<F>(
        sockets: Vec<Arc<UdpSocket>>,
        exit: &Arc<AtomicBool>,
        sender: PacketSender,
        recycler: Recycler<PinnedVec<Packet>>,
        bank_forks: Option<Arc<RwLock<BankForks>>>,
        modify: F,
    ) -> (Vec<JoinHandle<()>>, JoinHandle<()>)
    where
        F: Fn(&mut Packet) + Send + 'static,
    {
        let (packet_sender, packet_receiver) = channel();
        let streamers = sockets
            .into_iter()
            .map(|s| {
                streamer::receiver(
                    s,
                    &exit,
                    packet_sender.clone(),
                    recycler.clone(),
                    "packet_modifier",
                )
            })
            .collect();

        let modifier_hdl = Builder::new()
            .name("solana-tvu-fetch-stage-packet-modifier".to_string())
            .spawn(move || Self::modify_packets(packet_receiver, sender, bank_forks, modify))
            .unwrap();
        (streamers, modifier_hdl)
    }

    pub fn new(
        sockets: Vec<Arc<UdpSocket>>,
        forward_sockets: Vec<Arc<UdpSocket>>,
        repair_socket: Arc<UdpSocket>,
        sender: &PacketSender,
        bank_forks: Option<Arc<RwLock<BankForks>>>,
        exit: &Arc<AtomicBool>,
    ) -> Self {
        let recycler: PacketsRecycler = Recycler::warmed(100, 1024);

        let tvu_threads = sockets.into_iter().map(|socket| {
            streamer::receiver(
                socket,
                &exit,
                sender.clone(),
                recycler.clone(),
                "shred_fetch_stage",
            )
        });

        let (tvu_forwards_threads, fwd_thread_hdl) = Self::packet_modifier(
            forward_sockets,
            &exit,
            sender.clone(),
            recycler.clone(),
            bank_forks.clone(),
            |p| p.meta.forward = true,
        );

        let (repair_receiver, repair_handler) = Self::packet_modifier(
            vec![repair_socket],
            &exit,
            sender.clone(),
            recycler.clone(),
            bank_forks,
            |p| p.meta.repair = true,
        );

        let mut thread_hdls: Vec<_> = tvu_threads
            .chain(tvu_forwards_threads.into_iter())
            .collect();
        thread_hdls.extend(repair_receiver.into_iter());
        thread_hdls.push(fwd_thread_hdl);
        thread_hdls.push(repair_handler);

        Self { thread_hdls }
    }

    pub fn join(self) -> thread::Result<()> {
        for thread_hdl in self.thread_hdls {
            thread_hdl.join()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_ledger::shred::Shred;

    #[test]
    fn test_shred_filter() {
        solana_logger::setup();
        let mut shreds_received = ShredsReceived::default();
        let mut packet = Packet::default();
        let mut index_overrun = 0;
        let last_root = 0;
        let last_slot = 100;
        let slots_per_epoch = 10;
        // packet size is 0, so cannot get index
        ShredFetchStage::process_packet(
            &mut packet,
            &mut shreds_received,
            &mut index_overrun,
            last_root,
            last_slot,
            slots_per_epoch,
            &|_p| {},
        );
        assert_eq!(index_overrun, 1);
        assert!(packet.meta.discard);
        let shred = Shred::new_from_data(1, 3, 0, None, true, true, 0, 0, 0);
        shred.copy_to_packet(&mut packet);

        // rejected slot is 1, root is 3
        ShredFetchStage::process_packet(
            &mut packet,
            &mut shreds_received,
            &mut index_overrun,
            3,
            last_slot,
            slots_per_epoch,
            &|_p| {},
        );
        assert!(packet.meta.discard);

        // Accepted for 1,3
        ShredFetchStage::process_packet(
            &mut packet,
            &mut shreds_received,
            &mut index_overrun,
            last_root,
            last_slot,
            slots_per_epoch,
            &|_p| {},
        );
        assert!(!packet.meta.discard);

        // shreds_received should filter duplicate
        ShredFetchStage::process_packet(
            &mut packet,
            &mut shreds_received,
            &mut index_overrun,
            last_root,
            last_slot,
            slots_per_epoch,
            &|_p| {},
        );
        assert!(packet.meta.discard);

        let shred = Shred::new_from_data(1_000_000, 3, 0, None, true, true, 0, 0, 0);
        shred.copy_to_packet(&mut packet);

        // Slot 1 million is too high
        ShredFetchStage::process_packet(
            &mut packet,
            &mut shreds_received,
            &mut index_overrun,
            last_root,
            last_slot,
            slots_per_epoch,
            &|_p| {},
        );
        assert!(packet.meta.discard);

        let index = MAX_DATA_SHREDS_PER_SLOT as u32;
        let shred = Shred::new_from_data(5, index, 0, None, true, true, 0, 0, 0);
        shred.copy_to_packet(&mut packet);
        ShredFetchStage::process_packet(
            &mut packet,
            &mut shreds_received,
            &mut index_overrun,
            last_root,
            last_slot,
            slots_per_epoch,
            &|_p| {},
        );
        assert!(packet.meta.discard);
    }

    #[test]
    fn test_shred_offsets() {
        let shred = Shred::new_from_data(1, 3, 0, None, true, true, 0, 0, 0);
        let mut packet = Packet::default();
        shred.copy_to_packet(&mut packet);
        let mut index_overrun = 0;
        assert_eq!(
            Some((1, 3)),
            ShredFetchStage::get_slot_index(&packet, &mut index_overrun)
        );
    }
}

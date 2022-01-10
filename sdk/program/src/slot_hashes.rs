//! named accounts for synthesized data accounts for bank state, etc.
//!
//! this account carries the Bank's most recent bank hashes for some N parents
//!
use {
    crate::hash::Hash,
    std::{iter::FromIterator, ops::Deref},
};

pub use crate::clock::Slot;

pub const MAX_ENTRIES: usize = 512; // about 2.5 minutes to get your vote in

// This is to allow tests with custom slot hash expiry to avoid having to generate
// 512 blocks for such tests.
static mut NUM_ENTRIES: usize = MAX_ENTRIES;

pub fn get_entries() -> usize {
    unsafe { NUM_ENTRIES }
}

pub fn set_entries_for_tests_only(_entries: usize) {
    unsafe {
        NUM_ENTRIES = _entries;
    }
}

pub type SlotHash = (Slot, Hash);

#[repr(C)]
#[derive(Serialize, Deserialize, PartialEq, Debug, Default)]
pub struct SlotHashes(Vec<SlotHash>);

impl SlotHashes {
    pub fn add(&mut self, slot: Slot, hash: Hash) {
        match self.binary_search_by(|(probe, _)| slot.cmp(probe)) {
            Ok(index) => (self.0)[index] = (slot, hash),
            Err(index) => (self.0).insert(index, (slot, hash)),
        }
        (self.0).truncate(get_entries());
    }
    pub fn position(&self, slot: &Slot) -> Option<usize> {
        self.binary_search_by(|(probe, _)| slot.cmp(probe)).ok()
    }
    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub fn get(&self, slot: &Slot) -> Option<&Hash> {
        self.binary_search_by(|(probe, _)| slot.cmp(probe))
            .ok()
            .map(|index| &self[index].1)
    }
    pub fn new(slot_hashes: &[SlotHash]) -> Self {
        let mut slot_hashes = slot_hashes.to_vec();
        slot_hashes.sort_by(|(a, _), (b, _)| b.cmp(a));
        Self(slot_hashes)
    }
    pub fn slot_hashes(&self) -> &[SlotHash] {
        &self.0
    }
}

impl FromIterator<(Slot, Hash)> for SlotHashes {
    fn from_iter<I: IntoIterator<Item = (Slot, Hash)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl Deref for SlotHashes {
    type Target = Vec<SlotHash>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use {super::*, crate::hash::hash};

    #[test]
    fn test() {
        let mut slot_hashes = SlotHashes::new(&[(1, Hash::default()), (3, Hash::default())]);
        slot_hashes.add(2, Hash::default());
        assert_eq!(
            slot_hashes,
            SlotHashes(vec![
                (3, Hash::default()),
                (2, Hash::default()),
                (1, Hash::default()),
            ])
        );

        let mut slot_hashes = SlotHashes::new(&[]);
        for i in 0..MAX_ENTRIES + 1 {
            slot_hashes.add(
                i as u64,
                hash(&[(i >> 24) as u8, (i >> 16) as u8, (i >> 8) as u8, i as u8]),
            );
        }
        for i in 0..MAX_ENTRIES {
            assert_eq!(slot_hashes[i].0, (MAX_ENTRIES - i) as u64);
        }

        assert_eq!(slot_hashes.len(), MAX_ENTRIES);
    }
}

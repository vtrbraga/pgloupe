//! A fast hasher for maps keyed by small integers, which the log being read cannot attack.

use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, Hasher},
};

/// A map hashed with [`FastHasher`].
pub(crate) type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<FastHasher>>;

const MULTIPLIER: u64 = 0x517c_c1b7_2722_0a95;

/// One multiply per word; much cheaper than `SipHash` for ids and pids.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct FastHasher(u64);

impl FastHasher {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(MULTIPLIER);
    }
}

impl Hasher for FastHasher {
    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0u8; 8];
            word[..chunk.len()].copy_from_slice(chunk);
            self.add(u64::from_le_bytes(word));
        }
    }

    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }

    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }

    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }

    fn finish(&self) -> u64 {
        self.0.rotate_left(26)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works_as_a_map_hasher() {
        let mut map: FastMap<Option<u32>, u64> = FastMap::default();
        for pid in 0..1_000u32 {
            map.insert(Some(pid), u64::from(pid) * 2);
        }
        map.insert(None, 7);
        assert_eq!(map.get(&Some(500)), Some(&1_000));
        assert_eq!(map.get(&None), Some(&7));
        assert_eq!(map.len(), 1_001);
    }
}

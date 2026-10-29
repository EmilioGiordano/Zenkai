use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// Hashes cell coordinates, which the evaluation looks up for every cell it reads, in a
/// few nanoseconds instead of the default hasher's tens.
///
/// Unlike the default hasher it is not seeded, so a file could choose coordinates that
/// collide. The final mix spreads every input bit over the bits a table uses, and a sheet
/// holds at most 2^34 coordinates, so at most about 2^34 / buckets of them share a bucket:
/// a few thousand in a table of millions, which costs probes, not a hang.
#[derive(Default, Clone, Copy)]
pub(crate) struct CellHasher(u64);

pub(crate) type CellHashMap<K, V> = HashMap<K, V, BuildHasherDefault<CellHasher>>;

impl CellHasher {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(26) ^ word).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

impl Hasher for CellHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.add(u64::from(byte));
        }
    }

    fn write_u32(&mut self, value: u32) {
        self.add(u64::from(value));
    }

    fn write_i32(&mut self, value: i32) {
        self.add(u64::from(value as u32));
    }

    fn write_u64(&mut self, value: u64) {
        self.add(value);
    }

    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }

    // The finalizer of MurmurHash3, so the low bits a table indexes by depend on every bit.
    fn finish(&self) -> u64 {
        let mut hash = self.0;
        hash ^= hash >> 33;
        hash = hash.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        hash ^= hash >> 33;
        hash = hash.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
        hash ^ (hash >> 33)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::hash::{BuildHasher, BuildHasherDefault};

    use super::CellHasher;

    fn hash(key: (u32, i32, i32)) -> u64 {
        BuildHasherDefault::<CellHasher>::default().hash_one(key)
    }

    // Cells of a column, a row and a block all land in different buckets of a table sized
    // for them, nearly as a random hash would place them.
    #[test]
    fn neighbouring_cells_spread_over_the_buckets() {
        let cells: Vec<(u32, i32, i32)> = (1..=4096)
            .map(|row| (0, row, 1))
            .chain((1..=4096).map(|column| (0, 1, column)))
            .chain((1..=64).flat_map(|row| (1..=64).map(move |column| (1, row, column))))
            .collect();
        let buckets: HashSet<u64> = cells.iter().map(|&cell| hash(cell) & 0xFFFF).collect();
        // A random hash of 12288 keys into 65536 buckets fills about 11300 of them.
        assert!(buckets.len() > 10_800, "{}", buckets.len());
    }
}

//! Bit packing used by the file formats.

/// Bits needed to store indices into a palette of `n` entries, at least `min`.
pub fn bits_for(n: usize, min: u32) -> u32 {
    let needed = if n <= 1 { 0 } else { usize::BITS - (n - 1).leading_zeros() };
    needed.max(min)
}

/// Litematica packing: values follow one another with no gaps and may
/// straddle two 64-bit words.
pub fn tight_len(count: u64, bits: u32) -> usize {
    ((count * bits as u64).div_ceil(64)) as usize
}

pub fn tight_get(longs: &[i64], bits: u32, index: u64) -> u32 {
    let start = index * bits as u64;
    let word = (start >> 6) as usize;
    let offset = (start & 63) as u32;
    let mask = (1u64 << bits) - 1;
    let mut v = (longs[word] as u64) >> offset;
    if offset + bits > 64 {
        v |= (longs[word + 1] as u64) << (64 - offset);
    }
    (v & mask) as u32
}

pub struct TightWriter {
    longs: Vec<i64>,
    bits: u32,
    at: u64,
}

impl TightWriter {
    pub fn new(count: u64, bits: u32) -> Self {
        Self { longs: vec![0; tight_len(count, bits)], bits, at: 0 }
    }
    pub fn push(&mut self, value: u32) {
        let start = self.at * self.bits as u64;
        let word = (start >> 6) as usize;
        let offset = (start & 63) as u32;
        self.longs[word] = (self.longs[word] as u64 | ((value as u64) << offset)) as i64;
        if offset + self.bits > 64 {
            self.longs[word + 1] = (self.longs[word + 1] as u64 | ((value as u64) >> (64 - offset))) as i64;
        }
        self.at += 1;
    }
    pub fn finish(self) -> Vec<i64> {
        self.longs
    }
}

/// The same packing as `TightWriter`, handing each finished word on as soon
/// as it is full instead of keeping them all.
pub struct TightStream {
    bits: u32,
    word: u64,
    filled: u32,
}

impl TightStream {
    pub fn new(bits: u32) -> Self {
        Self { bits, word: 0, filled: 0 }
    }
    pub fn push(&mut self, value: u32, out: &mut impl FnMut(i64)) {
        let v = value as u64;
        self.word |= v << self.filled;
        if self.filled + self.bits >= 64 {
            out(self.word as i64);
            self.word = if self.filled + self.bits > 64 { v >> (64 - self.filled) } else { 0 };
            self.filled = self.filled + self.bits - 64;
        } else {
            self.filled += self.bits;
        }
    }
    /// Hand on the last, part-filled word.
    pub fn finish(self, out: &mut impl FnMut(i64)) {
        if self.filled > 0 {
            out(self.word as i64);
        }
    }
}

/// Game-style packing (used by Axiom): each 64-bit word holds as many whole
/// values as fit; values never straddle words.
pub fn padded_len(count: usize, bits: u32) -> usize {
    count.div_ceil((64 / bits) as usize)
}

pub fn padded_get(longs: &[i64], bits: u32, index: usize) -> u32 {
    let per = (64 / bits) as usize;
    let mask = (1u64 << bits) - 1;
    (((longs[index / per] as u64) >> ((index % per) as u32 * bits)) & mask) as u32
}

pub fn padded_pack(values: &[u32], bits: u32) -> Vec<i64> {
    let per = (64 / bits) as usize;
    let mut longs = vec![0i64; padded_len(values.len(), bits)];
    for (i, v) in values.iter().enumerate() {
        longs[i / per] = (longs[i / per] as u64 | ((*v as u64) << ((i % per) as u32 * bits))) as i64;
    }
    longs
}

/// Sponge schematic block data: each value as a variable-length integer.
pub fn varint_push(out: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        out.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// Read one value; returns it and the position after it.
pub fn varint_read(data: &[u8], mut at: usize) -> Option<(u32, usize)> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let b = *data.get(at)?;
        at += 1;
        value |= ((b & 0x7F) as u32) << shift;
        if b & 0x80 == 0 {
            return Some((value, at));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Packing as the values come gives the longs packing all at once gives.
    #[test]
    fn tight_stream_packs_as_the_writer_does() {
        for bits in [1u32, 2, 3, 5, 7, 12, 13, 31] {
            for count in [0u64, 1, 63, 64, 65, 1000] {
                let values: Vec<u32> = (0..count).map(|i| ((i * 2654435761) as u32) & ((1u64 << bits) - 1) as u32).collect();
                let mut whole = TightWriter::new(count, bits);
                values.iter().for_each(|v| whole.push(*v));
                let (mut streamed, mut stream) = (Vec::new(), TightStream::new(bits));
                let mut out = |l: i64| streamed.push(l);
                values.iter().for_each(|v| stream.push(*v, &mut out));
                stream.finish(&mut out);
                assert_eq!(whole.finish(), streamed, "{bits} bits, {count} values");
                assert_eq!(streamed.len(), tight_len(count, bits), "{bits} bits, {count} values");
            }
        }
    }

    #[test]
    fn bit_widths() {
        assert_eq!(bits_for(1, 2), 2);
        assert_eq!(bits_for(4, 2), 2);
        assert_eq!(bits_for(5, 2), 3);
        assert_eq!(bits_for(16, 4), 4);
        assert_eq!(bits_for(17, 4), 5);
        assert_eq!(bits_for(1, 0), 0);
    }

    #[test]
    fn tight_and_padded_packing_round_trip_at_every_width() {
        for bits in 2..=20u32 {
            let values: Vec<u32> = (0..1000u32).map(|i| (i.wrapping_mul(2654435761)) & ((1 << bits) - 1)).collect();
            let mut w = TightWriter::new(values.len() as u64, bits);
            values.iter().for_each(|v| w.push(*v));
            let longs = w.finish();
            assert_eq!(longs.len(), tight_len(1000, bits));
            for (i, v) in values.iter().enumerate() {
                assert_eq!(tight_get(&longs, bits, i as u64), *v, "tight {bits} bits at {i}");
            }
            let longs = padded_pack(&values, bits);
            for (i, v) in values.iter().enumerate() {
                assert_eq!(padded_get(&longs, bits, i), *v, "padded {bits} bits at {i}");
            }
        }
        // 4096 cells at 5 bits is the size seen in real Axiom files.
        assert_eq!(padded_len(4096, 5), 342);
        assert_eq!(padded_len(4096, 6), 410);
    }

    #[test]
    fn varints_round_trip() {
        let mut out = Vec::new();
        let values = [0u32, 1, 127, 128, 300, 16383, 16384, u32::MAX];
        values.iter().for_each(|v| varint_push(&mut out, *v));
        let mut at = 0;
        for v in values {
            let (got, next) = varint_read(&out, at).unwrap();
            assert_eq!(got, v);
            at = next;
        }
        assert_eq!(at, out.len());
        assert!(varint_read(&[0x80], 0).is_none());
    }
}

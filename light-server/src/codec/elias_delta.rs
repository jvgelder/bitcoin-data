#[derive(Default)]
struct BitWriter {
    bytes: Vec<u8>,
    bit_len: usize,
}

impl BitWriter {
    fn push_bit(&mut self, bit: bool) {
        if self.bit_len % 8 == 0 {
            self.bytes.push(0);
        }
        if bit {
            let byte = self.bit_len / 8;
            let shift = 7 - (self.bit_len % 8);
            self.bytes[byte] |= 1 << shift;
        }
        self.bit_len += 1;
    }

    fn push_bits(&mut self, value: u64, bits: u32) {
        for i in (0..bits).rev() {
            self.push_bit(((value >> i) & 1) == 1);
        }
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

struct BitReader<'a> {
    bytes: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, bit_pos: 0 }
    }

    fn read_bit(&mut self) -> Option<bool> {
        if self.bit_pos >= self.bytes.len() * 8 {
            return None;
        }
        let byte = self.bit_pos / 8;
        let shift = 7 - (self.bit_pos % 8);
        self.bit_pos += 1;
        Some(((self.bytes[byte] >> shift) & 1) == 1)
    }

    fn read_bits(&mut self, bits: u32) -> Option<u64> {
        let mut v = 0u64;
        for _ in 0..bits {
            v = (v << 1) | u64::from(self.read_bit()?);
        }
        Some(v)
    }
}

pub fn encode_elias_delta_values(values: &[u64]) -> anyhow::Result<Vec<u8>> {
    let mut w = BitWriter::default();
    for &value in values {
        encode_one(value, &mut w)?;
    }
    Ok(w.into_bytes())
}

fn encode_one(value: u64, w: &mut BitWriter) -> anyhow::Result<()> {
    anyhow::ensure!(value >= 1, "Elias-delta only encodes positive integers");
    let n_bits = 64 - value.leading_zeros();
    let len_bits = 32 - n_bits.leading_zeros();
    for _ in 0..(len_bits - 1) {
        w.push_bit(false);
    }
    w.push_bits(n_bits as u64, len_bits);
    if n_bits > 1 {
        w.push_bits(value ^ (1u64 << (n_bits - 1)), n_bits - 1);
    }
    Ok(())
}

pub fn decode_elias_delta_values(bytes: &[u8], count: usize) -> anyhow::Result<Vec<u64>> {
    let mut r = BitReader::new(bytes);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(decode_one(&mut r)?);
    }
    Ok(out)
}

fn decode_one(r: &mut BitReader<'_>) -> anyhow::Result<u64> {
    let mut zeros = 0u32;
    loop {
        match r.read_bit() {
            Some(false) => {
                zeros += 1;
                anyhow::ensure!(zeros <= 63, "invalid Elias-delta prefix");
            }
            Some(true) => break,
            None => anyhow::bail!("truncated Elias-delta prefix"),
        }
    }
    let mut n_bits = 1u64 << zeros;
    if zeros > 0 {
        n_bits |= r
            .read_bits(zeros)
            .ok_or_else(|| anyhow::anyhow!("truncated Elias-delta length"))?;
    }
    anyhow::ensure!((1..=64).contains(&n_bits), "invalid Elias-delta length");
    let tail_bits = (n_bits - 1) as u32;
    let tail = if tail_bits == 0 {
        0
    } else {
        r.read_bits(tail_bits)
            .ok_or_else(|| anyhow::anyhow!("truncated Elias-delta value"))?
    };
    Ok((1u64 << tail_bits) | tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_elias_delta() {
        let values = [
            1,
            2,
            3,
            4,
            5,
            17,
            255,
            256,
            257,
            65_535,
            1_000_000,
            u32::MAX as u64,
        ];
        let bytes = encode_elias_delta_values(&values).unwrap();
        let decoded = decode_elias_delta_values(&bytes, values.len()).unwrap();
        assert_eq!(decoded, values);
    }
}

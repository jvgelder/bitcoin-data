pub fn encode_leb128_values(values: &[u64]) -> Vec<u8> {
    let mut out = Vec::new();
    for &v in values {
        encode_one(v, &mut out);
    }
    out
}

fn encode_one(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

pub fn decode_leb128_values(bytes: &[u8], count: usize) -> anyhow::Result<Vec<u64>> {
    let mut out = Vec::with_capacity(count);
    let mut i = 0usize;
    for _ in 0..count {
        let mut shift = 0u32;
        let mut value = 0u64;
        loop {
            anyhow::ensure!(i < bytes.len(), "truncated LEB128 stream");
            let byte = bytes[i];
            i += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            anyhow::ensure!(shift < 64, "LEB128 value is too large");
        }
        out.push(value);
    }
    Ok(out)
}

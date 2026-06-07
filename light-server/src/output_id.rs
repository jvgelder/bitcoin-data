use crate::types::OutputIdHash;

pub fn output_id(output_ids: &[u8], output_id_bytes: u8, index: usize) -> anyhow::Result<&[u8]> {
    let len = output_id_bytes as usize;
    anyhow::ensure!(len > 0, "output_id_bytes must be non-zero");
    let start = index
        .checked_mul(len)
        .ok_or_else(|| anyhow::anyhow!("output id offset overflow"))?;
    let end = start
        .checked_add(len)
        .ok_or_else(|| anyhow::anyhow!("output id offset overflow"))?;
    anyhow::ensure!(end <= output_ids.len(), "output id index out of bounds");
    Ok(&output_ids[start..end])
}

pub fn choose_output_id_bytes(n: u64, collision_probability_log2: u32) -> u8 {
    if n <= 1 {
        return 1;
    }
    let n2_bits = 2 * (64 - (n - 1).leading_zeros());
    let bits = n2_bits + collision_probability_log2;
    bits.div_ceil(8).min(u8::MAX as u32) as u8
}

pub fn truncate_into_packed(
    full_hashes: &[OutputIdHash],
    output_id_bytes: u8,
) -> anyhow::Result<Vec<u8>> {
    let len = output_id_bytes as usize;
    anyhow::ensure!((1..=32).contains(&len), "output_id_bytes must be in 1..=32");
    let mut out = Vec::with_capacity(full_hashes.len() * len);
    for h in full_hashes {
        out.extend_from_slice(&h.as_bytes()[..len]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_access() {
        let ids = vec![1, 2, 3, 4, 5, 6];
        assert_eq!(output_id(&ids, 2, 1).unwrap(), &[3, 4]);
    }
}

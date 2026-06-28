//! Small shared binary decoding helpers.

pub fn read_32(bytes: &[u8], label: &str) -> anyhow::Result<[u8; 32]> {
    anyhow::ensure!(bytes.len() == 32, "invalid {label} length {}", bytes.len());
    let mut out = [0u8; 32];
    out.copy_from_slice(bytes);
    Ok(out)
}

pub fn read_u16_le(bytes: &[u8], label: &str) -> anyhow::Result<u16> {
    anyhow::ensure!(bytes.len() == 2, "invalid {label} length {}", bytes.len());
    let mut out = [0u8; 2];
    out.copy_from_slice(bytes);
    Ok(u16::from_le_bytes(out))
}

pub fn read_u32_le(bytes: &[u8], label: &str) -> anyhow::Result<u32> {
    anyhow::ensure!(bytes.len() == 4, "invalid {label} length {}", bytes.len());
    let mut out = [0u8; 4];
    out.copy_from_slice(bytes);
    Ok(u32::from_le_bytes(out))
}

pub fn read_u32_be(bytes: &[u8], label: &str) -> anyhow::Result<u32> {
    anyhow::ensure!(bytes.len() == 4, "invalid {label} length {}", bytes.len());
    let mut out = [0u8; 4];
    out.copy_from_slice(bytes);
    Ok(u32::from_be_bytes(out))
}

pub fn read_u64_be(bytes: &[u8], label: &str) -> anyhow::Result<u64> {
    anyhow::ensure!(bytes.len() == 8, "invalid {label} length {}", bytes.len());
    let mut out = [0u8; 8];
    out.copy_from_slice(bytes);
    Ok(u64::from_be_bytes(out))
}

pub fn read_u16_list(list: capnp::primitive_list::Reader<'_, u16>) -> Vec<u16> {
    let mut out = Vec::with_capacity(list.len() as usize);
    for i in 0..list.len() {
        out.push(list.get(i));
    }
    out
}

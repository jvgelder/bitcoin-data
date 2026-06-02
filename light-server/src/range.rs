use crate::{RANGE_MAGIC, RANGE_VERSION};

pub fn frame_range(messages: &[Vec<u8>]) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(RANGE_MAGIC);
    out.extend_from_slice(&RANGE_VERSION.to_le_bytes());
    out.extend_from_slice(&(messages.len() as u32).to_le_bytes());
    for msg in messages {
        let len: u32 = msg.len().try_into()?;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(msg);
    }
    Ok(out)
}

pub fn parse_range(bytes: &[u8]) -> anyhow::Result<Vec<&[u8]>> {
    anyhow::ensure!(bytes.len() >= 10, "range frame too short");
    anyhow::ensure!(&bytes[0..4] == RANGE_MAGIC, "bad range magic");
    let version = u16::from_le_bytes(bytes[4..6].try_into().unwrap());
    anyhow::ensure!(version == RANGE_VERSION, "unsupported range version: {version}");
    let count = u32::from_le_bytes(bytes[6..10].try_into().unwrap()) as usize;
    let mut pos = 10usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        anyhow::ensure!(pos + 4 <= bytes.len(), "truncated range item length");
        let len = u32::from_le_bytes(bytes[pos..pos+4].try_into().unwrap()) as usize;
        pos += 4;
        anyhow::ensure!(pos + len <= bytes.len(), "truncated range item body");
        out.push(&bytes[pos..pos+len]);
        pos += len;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_roundtrip() {
        let body = frame_range(&[b"a".to_vec(), b"bc".to_vec()]).unwrap();
        let parsed = parse_range(&body).unwrap();
        assert_eq!(parsed, vec![&b"a"[..], &b"bc"[..]]);
    }
}

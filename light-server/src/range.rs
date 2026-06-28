use crate::helper::{read_u16_le, read_u32_le};
use crate::{RANGE_MAGIC, RANGE_VERSION};

pub fn frame_range(messages: &[Vec<u8>]) -> anyhow::Result<Vec<u8>> {
    let count = u32::try_from(messages.len())?;
    let mut out = Vec::new();
    out.extend_from_slice(RANGE_MAGIC);
    out.extend_from_slice(&RANGE_VERSION.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
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
    let version = read_u16_le(&bytes[4..6], "range version")?;
    anyhow::ensure!(
        version == RANGE_VERSION,
        "unsupported range version: {version}"
    );
    let count = read_u32_le(&bytes[6..10], "range item count")? as usize;
    let mut pos = 10usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        anyhow::ensure!(pos + 4 <= bytes.len(), "truncated range item length");
        let len = read_u32_le(&bytes[pos..pos + 4], "range item length")? as usize;
        pos += 4;
        anyhow::ensure!(pos + len <= bytes.len(), "truncated range item body");
        out.push(&bytes[pos..pos + len]);
        pos += len;
    }
    anyhow::ensure!(pos == bytes.len(), "trailing bytes after range frame");
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

    #[test]
    fn range_rejects_trailing_bytes() {
        let mut body = frame_range(&[b"a".to_vec()]).unwrap();
        body.push(0);
        assert!(parse_range(&body).is_err());
    }
}

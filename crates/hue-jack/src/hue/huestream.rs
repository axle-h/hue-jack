//! HueStream v2 packet encoder.
//!
//! Layout: `"HueStream"` (9 bytes), version `0x02 0x00`, sequence (u8), reserved `0x00 0x00`,
//! colour space (`0x00` = RGB), reserved `0x00`, entertainment configuration id (36 ASCII bytes),
//! then per channel `channel_id (u8)` + R, G, B as big-endian u16. 52 header bytes + 7 per channel.

use thiserror::Error;

pub const HEADER_LEN: usize = 52;
pub const CHANNEL_LEN: usize = 7;
pub const MAX_CHANNELS: usize = 20;
const PROTOCOL: &[u8; 9] = b"HueStream";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EncodeError {
    #[error("entertainment configuration id must be 36 ASCII bytes, got {0}")]
    BadConfigId(usize),
    #[error("at most {MAX_CHANNELS} channels per packet, got {0}")]
    TooManyChannels(usize),
}

/// Encodes one RGB HueStream v2 packet into `out` (cleared first).
pub fn encode_into(
    out: &mut Vec<u8>,
    seq: u8,
    config_id: &str,
    channels: &[(u8, [u16; 3])],
) -> Result<(), EncodeError> {
    if config_id.len() != 36 || !config_id.is_ascii() {
        return Err(EncodeError::BadConfigId(config_id.len()));
    }
    if channels.len() > MAX_CHANNELS {
        return Err(EncodeError::TooManyChannels(channels.len()));
    }
    out.clear();
    out.reserve(HEADER_LEN + CHANNEL_LEN * channels.len());
    out.extend_from_slice(PROTOCOL);
    out.extend_from_slice(&[0x02, 0x00, seq, 0x00, 0x00, 0x00, 0x00]);
    out.extend_from_slice(config_id.as_bytes());
    for (id, [r, g, b]) in channels {
        out.push(*id);
        out.extend_from_slice(&r.to_be_bytes());
        out.extend_from_slice(&g.to_be_bytes());
        out.extend_from_slice(&b.to_be_bytes());
    }
    Ok(())
}

/// Encodes one RGB HueStream v2 packet.
pub fn encode(
    seq: u8,
    config_id: &str,
    channels: &[(u8, [u16; 3])],
) -> Result<Vec<u8>, EncodeError> {
    let mut out = Vec::new();
    encode_into(&mut out, seq, config_id, channels)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "1a8d99cc-967b-44f2-9202-43f976c0fa6b";

    #[test]
    fn golden_bytes() {
        let packet = encode(
            7,
            ID,
            &[(0, [0xffff, 0x0000, 0x1234]), (5, [0x0001, 0x8000, 0xabcd])],
        )
        .unwrap();
        let mut expected: Vec<u8> = vec![
            b'H', b'u', b'e', b'S', b't', b'r', b'e', b'a', b'm', // protocol
            0x02, 0x00, // version 2.0
            0x07, // sequence
            0x00, 0x00, // reserved
            0x00, // colour space RGB
            0x00, // reserved
        ];
        expected.extend_from_slice(b"1a8d99cc-967b-44f2-9202-43f976c0fa6b");
        expected.extend_from_slice(&[0x00, 0xff, 0xff, 0x00, 0x00, 0x12, 0x34]);
        expected.extend_from_slice(&[0x05, 0x00, 0x01, 0x80, 0x00, 0xab, 0xcd]);
        assert_eq!(packet, expected);
        assert_eq!(packet.len(), HEADER_LEN + 2 * CHANNEL_LEN);
    }

    #[test]
    fn empty_frame_is_header_only() {
        assert_eq!(encode(0, ID, &[]).unwrap().len(), HEADER_LEN);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(encode(0, "short", &[]), Err(EncodeError::BadConfigId(5)));
        let many = vec![(0u8, [0u16; 3]); 21];
        assert_eq!(encode(0, ID, &many), Err(EncodeError::TooManyChannels(21)));
    }
}

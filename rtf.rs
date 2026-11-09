//! Decompressing Outlook's compressed RTF body (`PR_RTF_COMPRESSED`).
//!
//! Many `.msg` files have no separate HTML body: Outlook stores the HTML
//! *inside* an RTF body, and compresses that RTF with a simple LZ77 scheme
//! called "LZFu". We need the text to find inline-image references
//! (`cid:...`). Spec: Microsoft [MS-OXRTFCP].

/// The dictionary starts pre-filled with these 207 bytes of common RTF.
const PREBUF: &[u8] = b"{\\rtf1\\ansi\\mac\\deff0\\deftab720{\\fonttbl;}{\\f0\\fnil \\froman \\fswiss \\fmodern \\fscript \\fdecor MS Sans SerifSymbolArialTimes New RomanCourier{\\colortbl\\red0\\green0\\blue0\r\n\\par \\pard\\plain\\f0\\fs20\\b\\i\\u\\tab\\tx";

const DICT_SIZE: usize = 4096;
const COMPRESSED: u32 = 0x7546_5A4C; // "LZFu"
const UNCOMPRESSED: u32 = 0x414C_454D; // "MELA"

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

/// Returns the decompressed RTF, or `None` if the data isn't valid.
pub fn decompress(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 16 {
        return None;
    }
    // 16-byte header: compressed size, raw size, type, CRC.
    let compressed_size = read_u32(data, 0) as usize;
    let raw_size = read_u32(data, 4) as usize;
    let end = data.len().min(compressed_size + 4);

    match read_u32(data, 8) {
        UNCOMPRESSED => {
            let body = &data[16..];
            return Some(body[..body.len().min(raw_size)].to_vec());
        }
        COMPRESSED => {}
        _ => return None,
    }

    let mut dict = [0u8; DICT_SIZE];
    dict[..PREBUF.len()].copy_from_slice(PREBUF);
    let mut write = PREBUF.len();
    let mut out = Vec::with_capacity(raw_size);
    let mut pos = 16;

    // Each control byte describes the next 8 items, lowest bit first:
    // 0 = one literal byte, 1 = a 2-byte reference into the dictionary.
    'outer: while pos < end {
        let control = data[pos];
        pos += 1;

        for bit in 0..8 {
            if pos >= end {
                break 'outer;
            }
            if control & (1 << bit) == 0 {
                let byte = data[pos];
                pos += 1;
                out.push(byte);
                dict[write] = byte;
                write = (write + 1) % DICT_SIZE;
            } else {
                if pos + 1 >= end {
                    break 'outer;
                }
                // Big-endian: 12 bits of offset, 4 bits of (length - 2).
                let word = u16::from_be_bytes([data[pos], data[pos + 1]]);
                pos += 2;
                let offset = (word >> 4) as usize;
                let length = (word & 0xF) as usize + 2;

                if offset == write {
                    break 'outer; // the end marker
                }
                for i in 0..length {
                    let byte = dict[(offset + i) % DICT_SIZE];
                    out.push(byte);
                    dict[write] = byte;
                    write = (write + 1) % DICT_SIZE;
                }
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prebuf_is_207_bytes() {
        assert_eq!(PREBUF.len(), 207);
    }

    #[test]
    fn decompresses_spec_example() {
        // Example 1 from [MS-OXRTFCP] section 4.1.
        let compressed = [
            0x2d, 0x00, 0x00, 0x00, 0x2b, 0x00, 0x00, 0x00, 0x4c, 0x5a, 0x46, 0x75, 0xf1, 0xc5,
            0xc7, 0xa7, 0x03, 0x00, 0x0a, 0x00, 0x72, 0x63, 0x70, 0x67, 0x31, 0x32, 0x35, 0x42,
            0x32, 0x0a, 0xf3, 0x20, 0x68, 0x65, 0x6c, 0x09, 0x00, 0x20, 0x62, 0x77, 0x05, 0xb0,
            0x6c, 0x64, 0x7d, 0x0a, 0x80, 0x0f, 0xa0,
        ];
        let text = decompress(&compressed).unwrap();
        assert_eq!(
            String::from_utf8(text).unwrap(),
            "{\\rtf1\\ansi\\ansicpg1252\\pard hello world}\r\n"
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(decompress(b"too short").is_none());
    }
}

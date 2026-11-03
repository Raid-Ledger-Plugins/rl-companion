//! Hostile LZ4 frames in the attributes section (ROK-2070 review MINOR-1,
//! MINOR-3): the frame decoder stays bounded by the declared section size,
//! refuses empty blocks and caps the block count.

use super::lsf_writer::{Codec, build_sections, meta_attrs, string_attr, write_lsf};
use super::writer::set_u32;
use crate::{LSF_MAX_SECTION_BYTES, LZ4_FRAME_MAX_BLOCKS, LsfError, read_lsf_attribute};

/// Frame header: magic, FLG 0x60 (v1, independent blocks), BD, checksum.
const FRAME_HEAD: [u8; 7] = [0x04, 0x22, 0x4D, 0x18, 0x60, 0x40, 0xA5];
const STORED: u32 = 0x8000_0000;

/// An LZ4 frame of `(word, payload)` blocks plus the end mark.
fn frame(blocks: impl IntoIterator<Item = (u32, Vec<u8>)>) -> Vec<u8> {
    let mut out = FRAME_HEAD.to_vec();
    for (word, payload) in blocks {
        out.extend_from_slice(&word.to_le_bytes());
        out.extend_from_slice(&payload);
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out
}

/// A valid stored resource whose attributes section is replaced by `frame`
/// with `declared` uncompressed bytes, under LZ4 flags (0x22). The other
/// sections keep `onDisk = 0`, so they still read as stored.
fn read_with_attr_frame(declared: u32, frame: &[u8]) -> Result<Option<String>, LsfError> {
    let s = build_sections(&meta_attrs(string_attr("GameID", "g")), true);
    let bytes = write_lsf(&s, Codec::Stored, true);
    let start = 64 + s.strings.len() + s.nodes.len();
    let mut out = bytes[..start].to_vec();
    out.extend_from_slice(frame);
    out.extend_from_slice(&bytes[start + s.attrs.len()..]);
    out[56] = 0x22;
    set_u32(&mut out, 40, declared);
    set_u32(&mut out, 44, frame.len() as u32);
    read_lsf_attribute(&out, "GameID")
}

fn attr_decode(reason: &str) -> Result<Option<String>, LsfError> {
    Err(LsfError::Decode {
        section: "attributes",
        reason: reason.to_owned(),
    })
}

rl_test! {
    fn refuses_a_frame_of_empty_blocks_at_the_first_one() {
        // ~13k one-byte blocks that each decode to nothing, under the 1 MiB cap.
        let empty = (0..13_000).map(|_| (1u32, vec![0x00]));
        let result = read_with_attr_frame(LSF_MAX_SECTION_BYTES as u32, &frame(empty));
        assert_eq!(result, attr_decode("LZ4 frame block decoded to 0 bytes"));
    }
}

rl_test! {
    fn refuses_a_frame_with_more_blocks_than_the_cap() {
        let one_byte = (0..=LZ4_FRAME_MAX_BLOCKS).map(|_| (1 | STORED, vec![b'a']));
        let result = read_with_attr_frame(LSF_MAX_SECTION_BYTES as u32, &frame(one_byte));
        let reason = format!("LZ4 frame has more than {LZ4_FRAME_MAX_BLOCKS} blocks");
        assert_eq!(result, attr_decode(&reason));
    }
}

rl_test! {
    fn refuses_a_stored_block_past_the_declared_size() {
        let result = read_with_attr_frame(16, &frame([(17 | STORED, vec![b'a'; 17])]));
        assert_eq!(result, attr_decode("LZ4 frame holds more than 16 bytes"));
    }
}

rl_test! {
    fn refuses_a_compressed_block_that_expands_past_the_declared_size() {
        let packed = lz4_flex::block::compress(&[0u8; 64 * 1024]);
        let result = read_with_attr_frame(16, &frame([(packed.len() as u32, packed)]));
        match result {
            Err(LsfError::Decode { section: "attributes", reason }) => {
                assert!(!reason.contains("decompressed to"), "decoded past the cap: {reason}");
            }
            other => panic!("expected an attributes decode error, got {other:?}"),
        }
    }
}

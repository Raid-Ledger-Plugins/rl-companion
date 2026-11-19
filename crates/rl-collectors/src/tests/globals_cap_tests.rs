//! `Globals.lsf` tables under their own size cap (ROK-2078). A real save's
//! Globals attributes section is ~3.16 MB decompressed, over the 1 MiB
//! `meta.lsf` cap, so every save read as `lsf-unreadable`. Synthetic bytes
//! only: many zero-length `Pad` records push the attributes section past
//! 1 MiB.

use super::lsf_writer::{Attr, Codec, INT32, LZ4, lsf};
use super::writer::set_u32;
use crate::{
    LSF_MAX_GLOBALS_SECTION_BYTES, LSF_MAX_SECTION_BYTES, LsfError, globals_lsf_parsed_version,
    lsf_attribute_len, lsf_parsed_version, read_lsf_attribute, read_lsf_attribute_bytes,
};

/// Attribute records are 12 bytes (V2): this many is just over 1 MiB.
const PAD_RECORDS: usize = (LSF_MAX_SECTION_BYTES as usize / 12) + 4_096;

/// A Globals-shaped resource: one `NewAge` blob then [`PAD_RECORDS`] records.
fn big_globals(codec: Codec) -> (Vec<u8>, Vec<u8>) {
    let blob = vec![0xA5u8; 4_096];
    let mut attrs = vec![Attr {
        name: "NewAge",
        type_id: 28,
        value: blob.clone(),
    }];
    attrs.extend((0..PAD_RECORDS).map(|_| Attr {
        name: "Pad",
        type_id: INT32,
        value: Vec::new(),
    }));
    (lsf(&attrs, codec, false), blob)
}

rl_test! {
    fn globals_attributes_over_one_mib_parse_under_the_globals_cap() {
        for codec in [Codec::Stored, LZ4, Codec::Zstd] {
            let (file, blob) = big_globals(codec);
            let attrs_size = u64::from(u32::from_le_bytes(file[40..44].try_into().expect("4")));
            assert!(attrs_size > LSF_MAX_SECTION_BYTES, "{codec:?}: section is over 1 MiB");
            assert_eq!(globals_lsf_parsed_version(&file), Some(7), "{codec:?}");
            let len = lsf_attribute_len(&file, "NewAge").expect("len");
            assert_eq!(len, Some(blob.len() as u64), "{codec:?}");
            let got = read_lsf_attribute_bytes(&file, "NewAge").expect("read");
            assert!(got.as_deref() == Some(&blob[..]), "{codec:?}: blob round-trips");
        }
    }
}

rl_test! {
    fn the_same_attributes_are_refused_under_the_meta_cap() {
        let (file, _) = big_globals(Codec::Stored);
        let size = u64::from(u32::from_le_bytes(file[40..44].try_into().expect("4")));
        let expected = LsfError::SectionTooLarge {
            section: "attributes",
            size,
            limit: LSF_MAX_SECTION_BYTES,
        };
        assert_eq!(read_lsf_attribute(&file, "NewAge"), Err(expected));
        assert_eq!(lsf_parsed_version(&file), None, "meta cap stays 1 MiB");
    }
}

rl_test! {
    fn globals_attributes_over_the_globals_cap_are_refused_before_reading() {
        let (mut file, _) = big_globals(Codec::Stored);
        let size = LSF_MAX_GLOBALS_SECTION_BYTES + 1;
        set_u32(&mut file, 40, size as u32);
        let expected = LsfError::SectionTooLarge {
            section: "attributes",
            size,
            limit: LSF_MAX_GLOBALS_SECTION_BYTES,
        };
        assert_eq!(lsf_attribute_len(&file, "NewAge"), Err(expected.clone()));
        assert_eq!(read_lsf_attribute_bytes(&file, "NewAge"), Err(expected));
        assert_eq!(globals_lsf_parsed_version(&file), None);
    }
}

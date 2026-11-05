//! Multi-part pak reader (ROK-2081). Synthetic packages only: part 0 comes
//! from the test writer, part 1 is hand-assembled entry data, and the part-1
//! records in part 0's table are rewritten to point at it.

use std::io::Cursor;

use super::writer::{
    ENTRY_OFFSET1, ENTRY_PART, ENTRY_SIZE_ON_DISK, ENTRY_UNCOMPRESSED, FileSpec, Method, get_u64,
    set_u16, set_u32, write_lspk, write_lspk_with,
};
use crate::lspk::ENTRY_LEN;
use crate::lspk_parts::{raw_cap, within_budget};
use crate::{
    LspkError, MAX_FILES, PAK_MAX_ENTRY_BYTES, PAK_MAX_PARTS, PAK_MAX_SET_BYTES,
    PAK_RAW_SLACK_BYTES, PakSet, part_file_name, read_lspk, read_pak_header,
};

const ENTRY_FLAGS: usize = ENTRY_PART + 1;

/// An entry stored in part 1: name, plain bytes, LZ4 or stored.
struct Remote {
    name: &'static str,
    plain: Vec<u8>,
    lz4: bool,
}

/// Part 0 holds `local` plus table records for `remote`; part 1 holds the
/// remote data. Part 0's header declares two parts.
fn two_parts(local: &[FileSpec<'_>], remote: &[Remote]) -> (Vec<u8>, Vec<u8>) {
    let mut specs: Vec<FileSpec<'_>> = local
        .iter()
        .map(|f| FileSpec {
            name: f.name,
            data: f.data,
            method: f.method,
        })
        .collect();
    specs.extend(remote.iter().map(|r| FileSpec {
        name: r.name,
        data: b"",
        method: Method::Stored,
    }));
    let mut part1 = Vec::new();
    let mut records = Vec::new();
    for r in remote {
        let stored = if r.lz4 {
            lz4_flex::block::compress(&r.plain)
        } else {
            r.plain.clone()
        };
        records.push((
            part1.len() as u32,
            stored.len() as u32,
            r.plain.len() as u32,
            if r.lz4 { 0x22 } else { 0 },
        ));
        part1.extend_from_slice(&stored);
    }
    let first = local.len();
    let mut part0 = write_lspk_with(&specs, |table| {
        for (i, (offset, on_disk, size, flags)) in records.iter().enumerate() {
            let at = (first + i) * ENTRY_LEN;
            set_u32(table, at + ENTRY_OFFSET1, *offset);
            table[at + ENTRY_PART] = 1;
            table[at + ENTRY_FLAGS] = *flags;
            set_u32(table, at + ENTRY_SIZE_ON_DISK, *on_disk);
            set_u32(table, at + ENTRY_UNCOMPRESSED, *size);
        }
    });
    set_u16(&mut part0, 38, 2);
    (part0, part1)
}

fn sample() -> (Vec<u8>, Vec<u8>) {
    two_parts(
        &[FileSpec {
            name: "Public/a.txt",
            data: b"in part zero",
            method: Method::Stored,
        }],
        &[
            Remote {
                name: "Public/b.bin",
                plain: b"part one, lz4, repetitive. ".repeat(12),
                lz4: true,
            },
            Remote {
                name: "Public/c.txt",
                plain: b"part one, stored".to_vec(),
                lz4: false,
            },
        ],
    )
}

fn open(parts: Vec<Vec<u8>>) -> Result<PakSet<Cursor<Vec<u8>>>, LspkError> {
    PakSet::open(parts.into_iter().map(Cursor::new).collect())
}

fn single(name: &'static str, data: &'static [u8], method: Method) -> Vec<u8> {
    write_lspk(&[FileSpec { name, data, method }])
}

/// Rewrite the first table record of a single-part package.
fn with_first_record(
    name: &'static str,
    data: &'static [u8],
    method: Method,
    edit: impl Fn(&mut Vec<u8>),
) -> Vec<u8> {
    write_lspk_with(&[FileSpec { name, data, method }], edit)
}

rl_test! {
    fn reads_entries_from_both_parts() {
        let (p0, p1) = sample();
        let mut set = open(vec![p0, p1]).expect("two-part set");
        assert_eq!(set.header().num_parts, 2);
        assert_eq!(set.parts_opened(), 2);
        let entries = set.entries().to_vec();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["Public/a.txt", "Public/b.bin", "Public/c.txt"]);
        assert_eq!(entries.iter().map(|e| e.archive_part).collect::<Vec<_>>(), [0, 1, 1]);
        assert_eq!(set.read_entry_limited(&entries[0], 1024).expect("part 0"), b"in part zero");
        assert_eq!(set.read_entry_limited(&entries[1], 1024).expect("part 1 lz4"), b"part one, lz4, repetitive. ".repeat(12));
        assert_eq!(set.read_entry_limited(&entries[2], 1024).expect("part 1 stored"), b"part one, stored");
        assert_eq!(set.bytes_read(), 12 + 27 * 12 + 16);
    }
}

rl_test! {
    fn a_single_part_save_reads_the_same_as_read_lspk() {
        let bytes = write_lspk(&[
            FileSpec { name: "x.bin", data: b"stored", method: Method::Stored },
            FileSpec { name: "y.bin", data: &[7u8; 300], method: Method::Lz4 },
            FileSpec { name: "z.bin", data: b"zstd raw", method: Method::ZstdRaw },
        ]);
        let package = read_lspk(&bytes).expect("slice reader");
        let mut set = open(vec![bytes.clone()]).expect("pak set");
        assert_eq!(set.entries(), package.entries());
        for entry in package.entries() {
            assert_eq!(set.read_entry_limited(entry, 1024).expect("pak set read"), package.read_entry(entry).expect("slice read"));
        }
    }
}

rl_test! {
    fn an_entry_in_a_part_that_was_not_opened_is_refused_and_the_rest_reads() {
        let (p0, _) = sample();
        let mut set = open(vec![p0]).expect("base part alone opens");
        let entries = set.entries().to_vec();
        assert_eq!(set.read_entry_limited(&entries[0], 1024).expect("part 0 still reads"), b"in part zero");
        assert_eq!(
            set.read_entry_limited(&entries[1], 1024),
            Err(LspkError::MissingArchivePart { name: "Public/b.bin".into(), part: 1, opened: 1 })
        );
    }
}

rl_test! {
    fn an_offset_past_the_end_of_its_part_is_truncated() {
        let (p0, p1) = sample();
        let len = p1.len() as u64;
        let mut set = open(vec![p0, p1]).expect("set");
        let mut entry = set.entries()[2].clone();
        entry.offset = len - 1;
        assert!(matches!(
            set.read_entry_limited(&entry, 1024),
            Err(LspkError::Truncated { what: "entry data", len: l, .. }) if l == len
        ), "an offset whose data runs past part 1 must be Truncated against part 1's length");
    }
}

rl_test! {
    fn the_part_count_is_capped_at_the_edge() {
        let base = single("a", b"a", Method::Stored);
        assert!(open(vec![base.clone(); PAK_MAX_PARTS]).is_ok(), "{PAK_MAX_PARTS} parts must open");
        assert_eq!(open(vec![base.clone(); PAK_MAX_PARTS + 1]).err(), Some(LspkError::PartCount { count: PAK_MAX_PARTS + 1, limit: PAK_MAX_PARTS }));
        assert_eq!(open(vec![]).err(), Some(LspkError::PartCount { count: 0, limit: PAK_MAX_PARTS }));
        let mut declared = base;
        set_u16(&mut declared, 38, PAK_MAX_PARTS as u16);
        assert_eq!(read_pak_header(&mut Cursor::new(declared.clone())).expect("16 declared").num_parts, 16);
        set_u16(&mut declared, 38, PAK_MAX_PARTS as u16 + 1);
        assert_eq!(read_pak_header(&mut Cursor::new(declared)).err(), Some(LspkError::PartCount { count: 17, limit: PAK_MAX_PARTS }));
    }
}

rl_test! {
    fn the_header_is_checked() {
        let mut bytes = single("a", b"a", Method::Stored);
        bytes[4] = 17;
        assert_eq!(open(vec![bytes.clone()]).err(), Some(LspkError::UnsupportedVersion(17)));
        bytes[0] = b'X';
        assert_eq!(open(vec![bytes]).err(), Some(LspkError::BadMagic(*b"XSPK")));
        assert!(matches!(open(vec![b"LSPK".to_vec()]).err(), Some(LspkError::Truncated { what: "header", .. })));
        let mut zero = single("a", b"a", Method::Stored);
        set_u16(&mut zero, 38, 0);
        assert_eq!(read_pak_header(&mut Cursor::new(zero)).expect("0 parts").num_parts, 1, "NumParts 0 reads as 1");
    }
}

rl_test! {
    fn the_entry_limit_is_enforced_at_the_edge() {
        let bytes = single("a.bin", &[1u8; 100], Method::Stored);
        let mut set = open(vec![bytes]).expect("set");
        let entry = set.entries()[0].clone();
        assert_eq!(set.read_entry_limited(&entry, 100).expect("at the limit").len(), 100);
        assert_eq!(set.read_entry_limited(&entry, 99), Err(LspkError::EntryTooLarge { name: "a.bin".into(), size: 100, limit: 99 }));
    }
}

rl_test! {
    fn the_default_entry_cap_refuses_one_byte_over_before_allocating() {
        let cap = PAK_MAX_ENTRY_BYTES;
        assert_eq!(cap, 256 * 1024 * 1024);
        for (claimed, over) in [(cap, false), (cap + 1, true)] {
            let bytes = with_first_record("big.bin", &[0u8; 300], Method::Lz4, |t| set_u32(t, ENTRY_UNCOMPRESSED, claimed as u32));
            let mut set = open(vec![bytes]).expect("set");
            let entry = set.entries()[0].clone();
            let got = set.read_entry_limited(&entry, PAK_MAX_ENTRY_BYTES);
            if over {
                assert_eq!(got, Err(LspkError::EntryTooLarge { name: "big.bin".into(), size: claimed, limit: cap }));
            } else {
                // At the cap the LZ4 ratio check refuses it before the decoder allocates.
                assert!(matches!(
                    &got,
                    Err(LspkError::Decompress { name, reason }) if name == "big.bin" && reason.ends_with(&format!("cannot hold {cap} bytes"))
                ), "claimed {claimed}: {got:?}");
            }
        }
    }
}

rl_test! {
    fn a_limit_above_the_caps_is_clamped_to_them() {
        let claimed = PAK_MAX_ENTRY_BYTES + 1;
        let bytes = with_first_record("big.bin", &[0u8; 300], Method::Lz4, |t| set_u32(t, ENTRY_UNCOMPRESSED, claimed as u32));
        let mut set = open(vec![bytes]).expect("set").with_set_limit(u64::MAX);
        assert_eq!(set.set_limit(), PAK_MAX_SET_BYTES, "a set limit above the default is clamped");
        let entry = set.entries()[0].clone();
        assert_eq!(
            set.read_entry_limited(&entry, u64::MAX),
            Err(LspkError::EntryTooLarge { name: "big.bin".into(), size: claimed, limit: PAK_MAX_ENTRY_BYTES }),
            "an entry limit above the default is clamped"
        );
        assert_eq!(open(vec![single("a", b"a", Method::Stored)]).expect("set").with_set_limit(10).set_limit(), 10, "tightening still works");
    }
}

rl_test! {
    fn an_entry_that_fails_to_decode_is_still_charged_to_the_set_budget() {
        let bytes = with_first_record("bad.bin", &[0u8; 300], Method::Lz4, |t| set_u32(t, ENTRY_UNCOMPRESSED, 5000));
        let mut set = open(vec![bytes]).expect("set").with_set_limit(5000);
        let entry = set.entries()[0].clone();
        assert!(matches!(set.read_entry_limited(&entry, 8192), Err(LspkError::Decompress { .. })), "5000 bytes from a tiny LZ4 block cannot decode");
        assert_eq!(set.bytes_read(), 5000, "the failed read was charged before its data was read");
        assert_eq!(
            set.read_entry_limited(&entry, 8192),
            Err(LspkError::SetBudgetExceeded { name: "bad.bin".into(), size: 5000, used: 5000, limit: 5000 }),
            "a retry is refused by the budget, before any I/O"
        );
    }
}

rl_test! {
    fn the_set_budget_is_enforced_at_the_edge() {
        let bytes = write_lspk(&[
            FileSpec { name: "a", data: &[1u8; 100], method: Method::Stored },
            FileSpec { name: "b", data: &[2u8; 50], method: Method::Stored },
        ]);
        let mut set = open(vec![bytes]).expect("set").with_set_limit(150);
        let entries = set.entries().to_vec();
        set.read_entry_limited(&entries[0], 1024).expect("100 of 150");
        set.read_entry_limited(&entries[1], 1024).expect("150 of 150: at the edge");
        assert_eq!(
            set.read_entry_limited(&entries[1], 1024),
            Err(LspkError::SetBudgetExceeded { name: "b".into(), size: 50, used: 150, limit: 150 })
        );
        assert_eq!(PAK_MAX_SET_BYTES, 512 * 1024 * 1024);
        assert!(within_budget(PAK_MAX_SET_BYTES - 10, 10, PAK_MAX_SET_BYTES), "exactly the default budget");
        assert!(!within_budget(PAK_MAX_SET_BYTES - 10, 11, PAK_MAX_SET_BYTES), "one byte over the default budget");
        assert!(!within_budget(u64::MAX, 1, u64::MAX), "overflow is over budget");
    }
}

rl_test! {
    fn on_disk_bytes_beyond_what_the_size_can_need_are_refused_at_the_edge() {
        assert_eq!(raw_cap(255), 255 + 1 + PAK_RAW_SLACK_BYTES);
        for (on_disk, over) in [(raw_cap(255), false), (raw_cap(255) + 1, true)] {
            let bytes = with_first_record("r.bin", &[3u8; 255], Method::Lz4, |t| set_u32(t, ENTRY_SIZE_ON_DISK, on_disk as u32));
            let mut set = open(vec![bytes]).expect("set");
            let entry = set.entries()[0].clone();
            let got = set.read_entry_limited(&entry, 1024);
            assert_eq!(matches!(got, Err(LspkError::RawTooLarge { .. })), over, "{on_disk} on-disk bytes: {got:?}");
        }
    }
}

rl_test! {
    fn the_file_table_is_bounded() {
        let bytes = single("a", b"a", Method::Stored);
        let list = get_u64(&bytes, 8) as usize;
        let mut many = bytes.clone();
        set_u32(&mut many, list, MAX_FILES + 1);
        assert_eq!(open(vec![many]).err(), Some(LspkError::TooManyFiles(MAX_FILES + 1)));
        let cap = raw_cap(ENTRY_LEN as u64) as u32;
        let mut over = bytes.clone();
        set_u32(&mut over, list + 4, cap + 1);
        assert!(matches!(open(vec![over]).err(), Some(LspkError::FileTable(m)) if m.contains("more than 1 entries can need")));
        let mut edge = bytes;
        set_u32(&mut edge, list + 4, cap);
        assert!(matches!(open(vec![edge]).err(), Some(LspkError::Truncated { what: "file table", .. })), "at the cap the size is accepted and only the file length refuses it");
    }
}

rl_test! {
    fn part_file_names_follow_the_lslib_convention() {
        assert_eq!(part_file_name("Gustav.pak", 0), "Gustav.pak");
        assert_eq!(part_file_name("Gustav.pak", 1), "Gustav_1.pak");
        assert_eq!(part_file_name("Patch2_Hotfix1.pak", 12), "Patch2_Hotfix1_12.pak");
        assert_eq!(part_file_name("noext", 2), "noext_2");
    }
}

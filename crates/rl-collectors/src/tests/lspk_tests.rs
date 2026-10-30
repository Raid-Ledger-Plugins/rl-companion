use super::fixtures::{PLACEHOLDER_WEBP, act2, synthetic_save};
use super::real_zstd::{REAL_ZSTD_FRAME, real_zstd_plain};
use super::writer::{
    ENTRY_OFFSET1, ENTRY_PART, ENTRY_SIZE_ON_DISK, ENTRY_UNCOMPRESSED, FileSpec, Method, get_u64,
    set_u16, set_u32, set_u64, write_lspk, write_lspk_with,
};
use crate::{CompressionMethod, LspkError, read_lspk};

fn sample() -> Vec<u8> {
    synthetic_save(act2().saveinfo, Some(PLACEHOLDER_WEBP))
}

fn one(name: &'static str, data: &'static [u8], method: Method) -> [FileSpec<'static>; 1] {
    [FileSpec { name, data, method }]
}

rl_test! {
    fn round_trips_every_supported_compression_method() {
        let plain = real_zstd_plain();
        let lz4_data = b"lz4 block data, quite repetitive. ".repeat(20);
        let raw_zstd = b"raw zstd block ".repeat(10);
        let bytes = write_lspk(&[
            FileSpec { name: "a.bin", data: b"stored bytes", method: Method::Stored },
            FileSpec { name: "Dir/b.bin", data: &lz4_data, method: Method::Lz4 },
            FileSpec { name: "c.bin", data: &raw_zstd, method: Method::ZstdRaw },
            FileSpec { name: "d.txt", data: plain.as_bytes(), method: Method::Precompressed(0x23, REAL_ZSTD_FRAME) },
        ]);
        let package = read_lspk(&bytes).expect("valid package");
        assert_eq!(package.version, 18);
        let names: Vec<&str> = package.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["a.bin", "Dir/b.bin", "c.bin", "d.txt"]);
        let methods: Vec<CompressionMethod> = package.entries().iter().map(|e| e.compression()).collect();
        assert_eq!(methods, [CompressionMethod::None, CompressionMethod::Lz4, CompressionMethod::Zstd, CompressionMethod::Zstd]);
        assert_eq!(package.read_file("a.bin").expect("stored"), b"stored bytes");
        assert_eq!(package.read_file("Dir/b.bin").expect("lz4"), lz4_data);
        assert_eq!(package.read_file("c.bin").expect("raw zstd"), raw_zstd);
        assert_eq!(package.read_file("d.txt").expect("real zstd frame"), plain.as_bytes());
    }
}

rl_test! {
    fn finds_entries_ignoring_ascii_case() {
        let bytes = sample();
        let package = read_lspk(&bytes).expect("valid package");
        assert!(package.find("saveinfo.JSON").is_some());
        assert_eq!(package.read_file("missing.bin"), Err(LspkError::NotFound("missing.bin".to_owned())));
    }
}

rl_test! {
    fn rejects_bad_magic() {
        let err = read_lspk(b"PK\x03\x04 this is a zip, not a save").unwrap_err();
        assert_eq!(err, LspkError::BadMagic(*b"PK\x03\x04"));
    }
}

rl_test! {
    fn rejects_unsupported_versions() {
        for version in [15_u32, 16, 17, 19] {
            let mut bytes = sample();
            set_u32(&mut bytes, 4, version);
            assert_eq!(read_lspk(&bytes).unwrap_err(), LspkError::UnsupportedVersion(version));
        }
    }
}

rl_test! {
    fn rejects_multi_part_packages() {
        let mut bytes = sample();
        set_u16(&mut bytes, 38, 2);
        assert_eq!(read_lspk(&bytes).unwrap_err(), LspkError::UnsupportedMultiPart(2));
    }
}

rl_test! {
    fn rejects_a_truncated_header() {
        let bytes = sample();
        for len in [0, 3, 4, 20, 39] {
            let err = read_lspk(&bytes[..len]).unwrap_err();
            assert!(matches!(err, LspkError::Truncated { what: "header", .. }), "len {len}: {err:?}");
        }
    }
}

rl_test! {
    fn rejects_a_truncated_file_table() {
        let bytes = sample();
        let err = read_lspk(&bytes[..bytes.len() - 3]).unwrap_err();
        assert!(matches!(err, LspkError::Truncated { what: "file table", .. }), "{err:?}");
    }
}

rl_test! {
    fn rejects_a_file_table_offset_past_the_end() {
        let mut bytes = sample();
        set_u64(&mut bytes, 8, u64::MAX - 2);
        let err = read_lspk(&bytes).unwrap_err();
        assert!(matches!(err, LspkError::Truncated { what: "file table header", end: u64::MAX, .. }), "{err:?}");
    }
}

rl_test! {
    fn rejects_too_many_files() {
        let mut bytes = sample();
        let table = get_u64(&bytes, 8) as usize;
        set_u32(&mut bytes, table, 70_000);
        assert_eq!(read_lspk(&bytes).unwrap_err(), LspkError::TooManyFiles(70_000));
    }
}

rl_test! {
    fn rejects_a_file_count_that_disagrees_with_the_table() {
        let mut bytes = sample();
        let table = get_u64(&bytes, 8) as usize;
        set_u32(&mut bytes, table, 2);
        let err = read_lspk(&bytes).unwrap_err();
        assert!(matches!(err, LspkError::FileTable(_)), "{err:?}");
    }
}

rl_test! {
    fn refuses_zlib_and_unknown_compression() {
        let bytes = write_lspk(&one("z.bin", b"zlib-ish", Method::Zlib));
        let package = read_lspk(&bytes).expect("table parses");
        assert_eq!(
            package.read_file("z.bin"),
            Err(LspkError::UnsupportedCompression { name: "z.bin".to_owned(), method: 1 })
        );
        let bytes = write_lspk(&one("u.bin", b"unknown", Method::Precompressed(0x27, b"unknown")));
        let package = read_lspk(&bytes).expect("table parses");
        assert_eq!(
            package.read_file("u.bin"),
            Err(LspkError::UnsupportedCompression { name: "u.bin".to_owned(), method: 7 })
        );
    }
}

rl_test! {
    fn refuses_entries_in_another_archive_part() {
        let bytes = write_lspk_with(&one("p.bin", b"data", Method::Stored), |t| t[ENTRY_PART] = 1);
        let package = read_lspk(&bytes).expect("table parses");
        assert_eq!(
            package.read_file("p.bin"),
            Err(LspkError::UnsupportedArchivePart { name: "p.bin".to_owned(), part: 1 })
        );
    }
}

rl_test! {
    fn rejects_entry_data_past_the_end() {
        let bytes = write_lspk_with(&one("s.bin", b"data", Method::Stored), |t| {
            set_u32(t, ENTRY_OFFSET1, 0xFFFF_0000);
        });
        let package = read_lspk(&bytes).expect("table parses");
        let err = package.read_file("s.bin").unwrap_err();
        assert!(matches!(err, LspkError::Truncated { what: "entry data", start: 0xFFFF_0000, .. }), "{err:?}");
        let bytes = write_lspk_with(&one("s.bin", b"data", Method::Stored), |t| {
            set_u32(t, ENTRY_SIZE_ON_DISK, 0x00FF_0000);
        });
        let package = read_lspk(&bytes).expect("table parses");
        let err = package.read_file("s.bin").unwrap_err();
        assert!(matches!(err, LspkError::Truncated { what: "entry data", .. }), "{err:?}");
    }
}

rl_test! {
    fn rejects_a_size_that_disagrees_with_the_data() {
        let data: &'static [u8] = b"zstd payload of a known length";
        let bytes = write_lspk_with(&one("z.bin", data, Method::ZstdRaw), |t| {
            set_u32(t, ENTRY_UNCOMPRESSED, data.len() as u32 + 5);
        });
        let package = read_lspk(&bytes).expect("table parses");
        assert_eq!(
            package.read_file("z.bin"),
            Err(LspkError::SizeMismatch {
                name: "z.bin".to_owned(),
                expected: data.len() as u64 + 5,
                actual: data.len() as u64,
            })
        );
        let bytes = write_lspk_with(&one("l.bin", data, Method::Lz4), |t| {
            set_u32(t, ENTRY_UNCOMPRESSED, 4);
        });
        let package = read_lspk(&bytes).expect("table parses");
        let err = package.read_file("l.bin").unwrap_err();
        assert!(matches!(err, LspkError::Decompress { .. } | LspkError::SizeMismatch { .. }), "{err:?}");
    }
}

rl_test! {
    fn rejects_corrupt_zstd() {
        let bytes = write_lspk(&one("g.bin", b"0123456789", Method::Precompressed(0x23, b"definitely not zstd")));
        let package = read_lspk(&bytes).expect("table parses");
        let err = package.read_file("g.bin").unwrap_err();
        assert!(matches!(err, LspkError::Decompress { .. }), "{err:?}");
    }
}

rl_test! {
    fn enforces_the_entry_size_limit_before_decoding() {
        let bytes = sample();
        let package = read_lspk(&bytes).expect("valid package");
        let entry = package.find("SaveInfo.json").expect("entry");
        let err = package.read_entry_limited(entry, 3).unwrap_err();
        assert_eq!(
            err,
            LspkError::EntryTooLarge { name: "SaveInfo.json".to_owned(), size: entry.size(), limit: 3 }
        );
    }
}

rl_test! {
    fn refuses_a_file_table_too_small_for_its_count_before_allocating() {
        let mut bytes = sample();
        let table = get_u64(&bytes, 8) as usize;
        // 65_536 entries is 17.8 MiB; the sample's few compressed bytes
        // cannot expand that far even at LZ4's 255:1 maximum.
        set_u32(&mut bytes, table, 65_536);
        let err = read_lspk(&bytes).unwrap_err();
        assert!(matches!(&err, LspkError::FileTable(m) if m.contains("cannot hold")), "{err:?}");
    }
}

rl_test! {
    fn refuses_a_zstd_window_larger_than_the_entry_before_decoding() {
        // Descriptor 0x00 (no content size), then a window byte, then one
        // last raw block of 2 bytes. 0x80 declares 64 MiB; 0x00 is 1 KiB.
        const BIG: &[u8] = &[0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x80, 0x11, 0x00, 0x00, b'{', b'}'];
        const SMALL: &[u8] = &[0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x00, 0x11, 0x00, 0x00, b'{', b'}'];
        let ok = write_lspk(&one("s.bin", b"{}", Method::Precompressed(0x23, SMALL)));
        let package = read_lspk(&ok).expect("table parses");
        assert_eq!(package.read_file("s.bin").expect("1 KiB window decodes"), b"{}");
        let bomb = write_lspk(&one("b.bin", b"{}", Method::Precompressed(0x23, BIG)));
        let package = read_lspk(&bomb).expect("table parses");
        let err = package.read_file("b.bin").unwrap_err();
        assert!(
            matches!(&err, LspkError::Decompress { reason, .. } if reason.contains("67108864-byte window")),
            "{err:?}"
        );
    }
}

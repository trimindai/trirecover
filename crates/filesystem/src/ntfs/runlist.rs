//! NTFS data-run-list decoder.
//!
//! Each run starts with a header byte:
//! - high nibble: number of bytes encoding the LCN offset (signed delta)
//! - low nibble : number of bytes encoding the run length (unsigned)
//!
//! A header byte of 0x00 ends the list. A run with `offset_bytes == 0` is a
//! sparse run (no on-disk allocation) — represented as `lcn: None`.
//!
//! The first non-sparse run's offset is absolute (delta from 0); subsequent
//! non-sparse runs are signed deltas from the previous run's start LCN.
//! Sparse runs do not advance the running LCN cursor.
//!
//! Returns volume-relative `RawRun`s measured in **clusters**. Translation to
//! sector-absolute, device-LBA `tr_core::DataRun`s happens in
//! [`crate::ntfs::mft::MftRecord::data_runs`], which knows the cluster size,
//! sector size, and partition start LBA.

use tr_core::{Error, Result};

/// One decoded entry from a run list.
///
/// `lcn == None` means the run is **sparse**: there is no on-disk allocation
/// for this VCN range; reading it yields zeroes. Sparse runs are kept in the
/// returned vector (rather than dropped) so callers can preserve VCN
/// alignment of subsequent non-sparse runs in fragmented files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawRun {
    /// Volume-relative starting cluster, or `None` for sparse runs.
    pub lcn: Option<i64>,
    /// Length in clusters.
    pub length_clusters: u64,
}

/// Decode a run list. See module docs for semantics.
pub fn decode(buf: &[u8]) -> Result<Vec<RawRun>> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let mut current_lcn: i64 = 0;

    while cursor < buf.len() {
        let header = buf[cursor];
        if header == 0 {
            break;
        }
        let len_bytes = (header & 0x0F) as usize;
        let off_bytes = (header >> 4) as usize;
        cursor += 1;

        if len_bytes == 0 {
            return Err(Error::corrupt(
                "runlist",
                cursor as u64,
                "length encoding length is 0",
            ));
        }
        if cursor + len_bytes + off_bytes > buf.len() {
            return Err(Error::UnexpectedEof {
                offset: cursor as u64,
                need: len_bytes + off_bytes,
                have: buf.len() - cursor,
            });
        }

        let length = read_unsigned_le(&buf[cursor..cursor + len_bytes]);
        cursor += len_bytes;

        let lcn = if off_bytes == 0 {
            None // sparse — does NOT advance current_lcn
        } else {
            let delta = read_signed_le(&buf[cursor..cursor + off_bytes]);
            current_lcn = current_lcn
                .checked_add(delta)
                .ok_or_else(|| Error::corrupt("runlist", cursor as u64, "LCN overflow"))?;
            if current_lcn < 0 {
                return Err(Error::corrupt("runlist", cursor as u64, "negative LCN"));
            }
            Some(current_lcn)
        };
        cursor += off_bytes;

        out.push(RawRun {
            lcn,
            length_clusters: length,
        });
    }
    Ok(out)
}

fn read_unsigned_le(bytes: &[u8]) -> u64 {
    let mut v = 0u64;
    for (i, b) in bytes.iter().enumerate() {
        v |= u64::from(*b) << (i * 8);
    }
    v
}

fn read_signed_le(bytes: &[u8]) -> i64 {
    let n = bytes.len();
    if n == 0 {
        return 0;
    }
    let mut v = 0u64;
    for (i, b) in bytes.iter().enumerate() {
        v |= u64::from(*b) << (i * 8);
    }
    // sign-extend
    let sign_bit = 1u64 << (n * 8 - 1);
    if v & sign_bit != 0 {
        let mask = !((1u128 << (n * 8)) - 1) as u64;
        v |= mask;
    }
    v as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_run() {
        // 0x21 0x18 0x34 0x56 → length_bytes=1, offset_bytes=2
        // length = 0x18 (24 clusters), offset = 0x5634 (delta from 0)
        let buf = [0x21, 0x18, 0x34, 0x56, 0x00];
        let runs = decode(&buf).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].length_clusters, 24);
        assert_eq!(runs[0].lcn, Some(0x5634));
    }

    #[test]
    fn two_runs_with_relative_offset() {
        // Run 1: length 0x10 starting at LCN 0x100
        // Run 2: length 0x08 with delta 0x20 → LCN 0x120
        let buf = [0x21, 0x10, 0x00, 0x01, 0x21, 0x08, 0x20, 0x00, 0x00];
        let runs = decode(&buf).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].lcn, Some(0x100));
        assert_eq!(runs[1].lcn, Some(0x100 + 0x20));
    }

    #[test]
    fn negative_offset_delta() {
        // length 0x08, delta = -0x10 (0xF0 sign-extended over 1 byte)
        let buf = [0x21, 0x08, 0x00, 0x01, 0x11, 0x04, 0xF0, 0x00];
        let runs = decode(&buf).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].lcn, Some(0x100));
        assert_eq!(runs[1].lcn, Some(0x100 - 0x10));
    }

    #[test]
    fn sparse_run_is_none() {
        // header 0x01: offset_bytes=0 → sparse run of length N
        let buf = [0x01, 0x05, 0x00];
        let runs = decode(&buf).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].lcn, None);
        assert_eq!(runs[0].length_clusters, 5);
    }

    #[test]
    fn sparse_does_not_advance_lcn() {
        // [data @ LCN 0x100 len 8] [sparse len 4] [data delta +0x10 → LCN 0x110]
        // If sparse advanced the cursor, the third run would land at 0x100+0x10
        // mistakenly using the sparse position as base.
        let buf = [
            0x21, 0x08, 0x00, 0x01, // run 1: len 8, off bytes=2, delta=0x100
            0x01, 0x04, // run 2: sparse, len 4
            0x11, 0x06, 0x10, // run 3: len 6, off bytes=1, delta=+0x10
            0x00,
        ];
        let runs = decode(&buf).unwrap();
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].lcn, Some(0x100));
        assert_eq!(runs[1].lcn, None);
        assert_eq!(runs[2].lcn, Some(0x110));
    }

    #[test]
    fn rejects_truncated() {
        let buf = [0x21, 0x10]; // says length=1, offset=2 but cuts off
        let r = decode(&buf);
        assert!(r.is_err());
    }
}

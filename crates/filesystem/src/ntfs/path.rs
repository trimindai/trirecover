//! NTFS path reconstruction.
//!
//! Each `$FILE_NAME` attribute in an MFT record names *one* link of the file
//! and points to the parent directory's MFT record number. To rebuild the
//! full path of a record we walk the parent chain upward until we hit the
//! volume root (record 5), splicing the names in reverse.
//!
//! [`PathResolver`] holds an in-memory map of `record_number → (parent, name)`
//! built up during the single MFT scan, then resolves any record on demand.
//! Resolutions are memoized; cycles and missing parents are handled
//! defensively (the result is rooted under `$Orphan/` instead of producing an
//! infinite path).

use crate::ntfs::attribute::FileName;
use std::collections::HashMap;

/// MFT record number of the volume root directory. NTFS reserves it.
pub const ROOT_RECORD: u64 = 5;

/// Cap on the parent walk to keep a corrupted MFT from sending us into an
/// effectively infinite loop. Real Windows directory trees never come close.
const MAX_DEPTH: usize = 256;

#[derive(Debug, Default)]
pub struct PathResolver {
    /// `record → (parent_record, leaf_name)`.
    entries: HashMap<u64, (u64, String)>,
    /// Memoized full paths.
    cache: HashMap<u64, String>,
}

impl PathResolver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one MFT record's best $FILE_NAME entry. Call this once per
    /// record (file or directory) during the MFT scan.
    pub fn register(&mut self, record: u64, fname: &FileName) {
        // Don't overwrite the root entry — its parent is itself in NTFS, and
        // we never want a stray name attached to it.
        if record == ROOT_RECORD {
            return;
        }
        self.entries
            .insert(record, (fname.parent_record, fname.name.clone()));
    }

    /// Resolve `record` to a forward-slash path rooted at `/`.
    /// Returns `/` for the root itself, and a `$Orphan/<leaf>` path for
    /// records whose parent chain breaks before reaching the root.
    pub fn resolve(&mut self, record: u64) -> String {
        if let Some(hit) = self.cache.get(&record) {
            return hit.clone();
        }
        let path = self.resolve_uncached(record);
        self.cache.insert(record, path.clone());
        path
    }

    fn resolve_uncached(&self, record: u64) -> String {
        if record == ROOT_RECORD {
            return "/".to_string();
        }

        let mut parts: Vec<&str> = Vec::new();
        let mut seen: HashMap<u64, ()> = HashMap::new();
        let mut cursor = record;
        let mut orphan = false;

        for _ in 0..MAX_DEPTH {
            if cursor == ROOT_RECORD {
                break;
            }
            if seen.insert(cursor, ()).is_some() {
                // Cycle — bail out and tag as orphan.
                orphan = true;
                break;
            }
            let Some((parent, name)) = self.entries.get(&cursor) else {
                // Missing record — chain is broken.
                orphan = true;
                break;
            };
            parts.push(name.as_str());
            cursor = *parent;
        }

        // If we exited the loop without hitting ROOT_RECORD and without
        // marking orphan, we exhausted MAX_DEPTH.
        if !orphan && cursor != ROOT_RECORD {
            orphan = true;
        }

        let mut path = if orphan {
            String::from("/$Orphan")
        } else {
            String::new()
        };
        for part in parts.iter().rev() {
            path.push('/');
            path.push_str(part);
        }
        if path.is_empty() {
            "/".to_string()
        } else {
            path
        }
    }

    #[must_use]
    pub fn known_records(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fname(parent: u64, name: &str) -> FileName {
        FileName {
            parent_record: parent,
            namespace: 1,
            name: name.to_string(),
        }
    }

    #[test]
    fn root_resolves_to_slash() {
        let mut r = PathResolver::new();
        assert_eq!(r.resolve(ROOT_RECORD), "/");
    }

    #[test]
    fn single_level_under_root() {
        let mut r = PathResolver::new();
        r.register(64, &fname(ROOT_RECORD, "hello.txt"));
        assert_eq!(r.resolve(64), "/hello.txt");
    }

    #[test]
    fn nested_path() {
        let mut r = PathResolver::new();
        r.register(40, &fname(ROOT_RECORD, "Users"));
        r.register(50, &fname(40, "alice"));
        r.register(60, &fname(50, "Documents"));
        r.register(70, &fname(60, "report.pdf"));
        assert_eq!(r.resolve(70), "/Users/alice/Documents/report.pdf");
    }

    #[test]
    fn missing_parent_yields_orphan() {
        let mut r = PathResolver::new();
        r.register(70, &fname(60, "lone.bin"));
        // parent 60 was never registered.
        assert_eq!(r.resolve(70), "/$Orphan/lone.bin");
    }

    #[test]
    fn cycle_does_not_loop_forever() {
        let mut r = PathResolver::new();
        r.register(70, &fname(80, "a"));
        r.register(80, &fname(70, "b"));
        let p = r.resolve(70);
        assert!(p.starts_with("/$Orphan/"), "got {p}");
    }

    #[test]
    fn root_register_is_ignored() {
        let mut r = PathResolver::new();
        r.register(ROOT_RECORD, &fname(ROOT_RECORD, "."));
        assert_eq!(r.resolve(ROOT_RECORD), "/");
    }

    #[test]
    fn resolve_is_memoized() {
        let mut r = PathResolver::new();
        r.register(40, &fname(ROOT_RECORD, "Users"));
        r.register(50, &fname(40, "alice"));
        assert_eq!(r.resolve(50), "/Users/alice");
        // Mutate the entry — memoized result must still be returned.
        r.entries.insert(50, (40, "bob".to_string()));
        assert_eq!(r.resolve(50), "/Users/alice");
    }

    #[test]
    fn deep_chain_caps_at_max_depth() {
        let mut r = PathResolver::new();
        // Build a chain that's MAX_DEPTH+10 long, never reaching ROOT_RECORD.
        // record 1000 → parent 1001 → parent 1002 ...
        for i in 0..(MAX_DEPTH as u64 + 10) {
            r.register(1000 + i, &fname(1000 + i + 1, "x"));
        }
        let p = r.resolve(1000);
        assert!(p.starts_with("/$Orphan/"), "got {p}");
    }
}

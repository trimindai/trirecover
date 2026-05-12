//! Scan strategy descriptions and defaults.
//!
//! Each [`RecoveryStrategy`] maps to a specific sequence of pipeline phases
//! (see [`crate::pipeline::ScanPipeline`]):
//!
//! | Strategy      | Partition table | FS metadata | Carve unalloc | Carve whole disk |
//! |---------------|:-:|:-:|:-:|:-:|
//! | Quick         | yes | yes | no  | no  |
//! | Deep          | yes | yes | yes | no  |
//! | Raw           | no  | no  | no  | yes |
//! | Partition     | yes (reconstruct) | yes | no | no |
//! | Formatted     | no  | no  | no  | yes |
//! | CorruptedFs   | yes (lenient) | yes (lenient) | yes | no |

use tr_core::RecoveryStrategy;

/// User-facing description of each strategy.
#[must_use]
pub fn description(s: RecoveryStrategy) -> &'static str {
    match s {
        RecoveryStrategy::Quick => {
            "Scans filesystem metadata (MFT, FAT directory) to find recently \
             deleted files. Fastest option — takes seconds to minutes."
        }
        RecoveryStrategy::Deep => {
            "Quick scan + signature-based carving of unallocated space. Finds \
             files that no longer appear in the filesystem. Medium speed."
        }
        RecoveryStrategy::Raw => {
            "Pure signature carving across the entire device, ignoring all \
             filesystem structures. Use when the filesystem is completely \
             unreadable. Slowest but most thorough."
        }
        RecoveryStrategy::Partition => {
            "Attempts to reconstruct missing or damaged partition tables, then \
             performs a Quick scan on each discovered partition."
        }
        RecoveryStrategy::Formatted => {
            "Treats the drive as if it was recently formatted. Ignores all \
             existing metadata and carves the entire surface. Similar to Raw \
             but may apply format-specific heuristics."
        }
        RecoveryStrategy::CorruptedFs => {
            "Best-effort scan of a corrupted filesystem. Reads whatever \
             metadata is still intact, then carves unallocated space to \
             recover files the damaged metadata missed."
        }
    }
}

/// Estimated relative speed (1 = fastest, 5 = slowest).
#[must_use]
pub fn speed_rating(s: RecoveryStrategy) -> u8 {
    match s {
        RecoveryStrategy::Quick => 1,
        RecoveryStrategy::Partition => 2,
        RecoveryStrategy::Deep => 3,
        RecoveryStrategy::CorruptedFs => 4,
        RecoveryStrategy::Formatted | RecoveryStrategy::Raw => 5,
    }
}

/// Recommended strategy based on the user's scenario.
#[must_use]
pub fn recommend(
    has_valid_partition_table: bool,
    has_valid_filesystem: bool,
    was_formatted: bool,
) -> RecoveryStrategy {
    if was_formatted {
        return RecoveryStrategy::Formatted;
    }
    if !has_valid_partition_table {
        return RecoveryStrategy::Partition;
    }
    if !has_valid_filesystem {
        return RecoveryStrategy::CorruptedFs;
    }
    // Default: Deep gives the best balance of speed and coverage.
    RecoveryStrategy::Deep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_strategies_have_descriptions() {
        for s in [
            RecoveryStrategy::Quick,
            RecoveryStrategy::Deep,
            RecoveryStrategy::Raw,
            RecoveryStrategy::Partition,
            RecoveryStrategy::Formatted,
            RecoveryStrategy::CorruptedFs,
        ] {
            assert!(!description(s).is_empty());
            assert!(speed_rating(s) >= 1 && speed_rating(s) <= 5);
        }
    }

    #[test]
    fn recommend_formatted() {
        assert_eq!(recommend(true, true, true), RecoveryStrategy::Formatted);
    }

    #[test]
    fn recommend_deep_for_normal() {
        assert_eq!(recommend(true, true, false), RecoveryStrategy::Deep);
    }

    #[test]
    fn recommend_partition_recovery() {
        assert_eq!(recommend(false, false, false), RecoveryStrategy::Partition);
    }

    #[test]
    fn recommend_corrupted_fs() {
        assert_eq!(recommend(true, false, false), RecoveryStrategy::CorruptedFs);
    }
}

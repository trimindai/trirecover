//! FAT table cluster-chain helpers (used internally by `Fat32Volume`).

#[must_use]
pub fn is_end_of_chain(entry: u32) -> bool {
    entry >= 0x0FFF_FFF8
}

#[must_use]
pub fn is_bad_cluster(entry: u32) -> bool {
    entry == 0x0FFF_FFF7
}

#[must_use]
pub fn is_free(entry: u32) -> bool {
    entry == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_values() {
        assert!(is_free(0));
        assert!(!is_free(2));
        assert!(is_bad_cluster(0x0FFFFFF7));
        assert!(is_end_of_chain(0x0FFFFFF8));
        assert!(is_end_of_chain(0x0FFFFFFF));
    }
}

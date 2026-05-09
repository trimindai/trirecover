//! SMART query trait. Concrete implementations live in `windows.rs` /
//! `linux.rs` (and may simply return `Unknown` where unsupported).

use async_trait::async_trait;
use tr_core::{Result, SmartReport};

#[async_trait]
pub trait SmartProvider: Send + Sync + std::fmt::Debug {
    async fn query(&self, drive_path: &str) -> Result<SmartReport>;
}

/// Classify a SMART report into an overall health bucket.
#[must_use]
pub fn classify(report: &SmartReport) -> tr_core::SmartHealth {
    use tr_core::SmartHealth;

    if let Some(reall) = report.reallocated_sectors {
        if reall > 100 {
            return SmartHealth::Failing;
        }
        if reall > 0 {
            return SmartHealth::Caution;
        }
    }
    if let Some(pending) = report.pending_sectors {
        if pending > 0 {
            return SmartHealth::Caution;
        }
    }
    if let Some(temp) = report.temperature_c {
        if temp > 70 {
            return SmartHealth::Caution;
        }
    }
    if let Some(wear) = report.wear_leveling_remaining {
        if wear < 10 {
            return SmartHealth::Failing;
        }
        if wear < 30 {
            return SmartHealth::Caution;
        }
    }
    SmartHealth::Ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tr_core::SmartHealth;

    fn empty() -> SmartReport {
        SmartReport {
            drive_path: "x".into(),
            overall: SmartHealth::Unknown,
            temperature_c: None,
            power_on_hours: None,
            reallocated_sectors: None,
            pending_sectors: None,
            wear_leveling_remaining: None,
            raw_attributes: vec![],
            captured_at: Utc::now(),
        }
    }

    #[test]
    fn ok_when_all_clean() {
        assert_eq!(classify(&empty()), SmartHealth::Ok);
    }

    #[test]
    fn caution_on_reallocation() {
        let r = SmartReport {
            reallocated_sectors: Some(5),
            ..empty()
        };
        assert_eq!(classify(&r), SmartHealth::Caution);
    }

    #[test]
    fn failing_on_heavy_reallocation() {
        let r = SmartReport {
            reallocated_sectors: Some(500),
            ..empty()
        };
        assert_eq!(classify(&r), SmartHealth::Failing);
    }

    #[test]
    fn caution_on_high_temp() {
        let r = SmartReport {
            temperature_c: Some(75),
            ..empty()
        };
        assert_eq!(classify(&r), SmartHealth::Caution);
    }

    #[test]
    fn failing_on_low_wear_remaining() {
        let r = SmartReport {
            wear_leveling_remaining: Some(5),
            ..empty()
        };
        assert_eq!(classify(&r), SmartHealth::Failing);
    }
}

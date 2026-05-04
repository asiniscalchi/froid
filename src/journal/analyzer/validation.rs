use chrono::NaiveDate;

use super::types::AnalyzerError;

pub(crate) fn validate_limit(limit: u32, max: u32) -> Result<u32, AnalyzerError> {
    if limit == 0 {
        return Err(AnalyzerError::InvalidArgument("limit must be > 0".into()));
    }
    if limit > max {
        return Err(AnalyzerError::LimitTooLarge { max });
    }
    Ok(limit)
}

pub(crate) fn validate_optional_range(
    from: Option<NaiveDate>,
    to_exclusive: Option<NaiveDate>,
) -> Result<(), AnalyzerError> {
    if let (Some(from), Some(to)) = (from, to_exclusive)
        && to <= from
    {
        return Err(AnalyzerError::InvalidArgument(
            "to_date_exclusive must be greater than from_date".into(),
        ));
    }
    Ok(())
}

pub(crate) fn validate_range(
    from: NaiveDate,
    to_exclusive: NaiveDate,
) -> Result<(), AnalyzerError> {
    if to_exclusive <= from {
        return Err(AnalyzerError::InvalidArgument(
            "to_date_exclusive must be greater than from_date".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_limit_rejects_zero() {
        let err = validate_limit(0, 10).unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }

    #[test]
    fn validate_limit_rejects_above_max() {
        let err = validate_limit(11, 10).unwrap_err();
        assert!(matches!(err, AnalyzerError::LimitTooLarge { max: 10 }));
    }

    #[test]
    fn validate_limit_accepts_valid_values() {
        assert_eq!(validate_limit(1, 10).unwrap(), 1);
        assert_eq!(validate_limit(10, 10).unwrap(), 10);
    }

    #[test]
    fn validate_optional_range_accepts_open_bounds() {
        assert!(validate_optional_range(None, None).is_ok());
        assert!(
            validate_optional_range(Some(NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()), None)
                .is_ok()
        );
        assert!(
            validate_optional_range(None, Some(NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()))
                .is_ok()
        );
    }

    #[test]
    fn validate_optional_range_rejects_inverted() {
        let err = validate_optional_range(
            Some(NaiveDate::from_ymd_opt(2026, 4, 29).unwrap()),
            Some(NaiveDate::from_ymd_opt(2026, 4, 28).unwrap()),
        )
        .unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }

    #[test]
    fn validate_range_rejects_equal_bounds() {
        let day = NaiveDate::from_ymd_opt(2026, 4, 28).unwrap();
        let err = validate_range(day, day).unwrap_err();
        assert!(matches!(err, AnalyzerError::InvalidArgument(_)));
    }
}

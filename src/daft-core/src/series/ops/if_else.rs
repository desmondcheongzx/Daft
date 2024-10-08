use common_error::DaftResult;

use super::cast_series_to_supertype;
use crate::series::Series;

impl Series {
    pub fn if_else(&self, other: &Self, predicate: &Self) -> DaftResult<Self> {
        let casted_series = cast_series_to_supertype(&[self, other])?;
        assert!(casted_series.len() == 2);

        let if_true = &casted_series[0];
        let if_false = &casted_series[1];

        if_true.inner.if_else(if_false, predicate)
    }
}

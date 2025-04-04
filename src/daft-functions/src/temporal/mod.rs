pub mod truncate;

use common_error::{DaftError, DaftResult};
use daft_core::{
    prelude::{DataType, Field, Schema, TimeUnit},
    series::Series,
};
use daft_dsl::{
    functions::{ScalarFunction, ScalarUDF},
    ExprRef,
};
use serde::{Deserialize, Serialize};

macro_rules! impl_temporal {
    // pyo3 macro can't handle any expressions other than a 'literal', so we have to redundantly pass it in via $py_name
    ($name:ident, $dt:ident, $py_name:literal, $dtype:ident) => {
        paste::paste! {
            #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
            pub struct $name;

            #[typetag::serde]
            impl ScalarUDF for $name {
                fn as_any(&self) -> &dyn std::any::Any {
                    self
                }

                fn name(&self) -> &'static str {
                    stringify!([ < $name:snake:lower > ])
                }

                fn to_field(&self, inputs: &[ExprRef], schema: &Schema) -> DaftResult<Field> {
                    match inputs {
                        [input] => match input.to_field(schema) {
                            Ok(field) if field.dtype.is_temporal() => {
                                Ok(Field::new(field.name, DataType::$dtype))
                            }
                            Ok(field) => Err(DaftError::TypeError(format!(
                                "Expected input to {} to be temporal, got {}",
                                self.name(),
                                field.dtype
                            ))),
                            Err(e) => Err(e),
                        },
                        _ => Err(DaftError::SchemaMismatch(format!(
                            "Expected 1 input arg, got {}",
                            inputs.len()
                        ))),
                    }
                }

                fn evaluate(&self, inputs: &[Series]) -> DaftResult<Series> {
                    match inputs {
                        [input] => input.$dt(),
                        _ => Err(DaftError::ValueError(format!(
                            "Expected 1 input arg, got {}",
                            inputs.len()
                        ))),
                    }
                }
            }

            #[must_use]
            pub fn $dt(input: ExprRef) -> ExprRef {
                ScalarFunction::new($name {}, vec![input]).into()
            }
        }
    };
}

impl_temporal!(Date, dt_date, "dt_date", Date);
impl_temporal!(Day, dt_day, "dt_day", UInt32);
impl_temporal!(Hour, dt_hour, "dt_hour", UInt32);
impl_temporal!(DayOfWeek, dt_day_of_week, "dt_day_of_week", UInt32);
impl_temporal!(DayOfYear, dt_day_of_year, "dt_day_of_year", UInt32);
impl_temporal!(Minute, dt_minute, "dt_minute", UInt32);
impl_temporal!(Month, dt_month, "dt_month", UInt32);
impl_temporal!(Second, dt_second, "dt_second", UInt32);
impl_temporal!(Millisecond, dt_millisecond, "dt_millisecond", UInt32);
impl_temporal!(Microsecond, dt_microsecond, "dt_microsecond", UInt32);
impl_temporal!(Nanosecond, dt_nanosecond, "dt_nanosecond", UInt32);
impl_temporal!(Year, dt_year, "dt_year", Int32);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Time;

#[typetag::serde]
impl ScalarUDF for Time {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &'static str {
        "time"
    }

    fn to_field(&self, inputs: &[ExprRef], schema: &Schema) -> DaftResult<Field> {
        match inputs {
            [input] => match input.to_field(schema) {
                Ok(field) => match field.dtype {
                    DataType::Time(_) => Ok(field),
                    DataType::Timestamp(tu, _) => {
                        let tu = match tu {
                            TimeUnit::Nanoseconds => TimeUnit::Nanoseconds,
                            _ => TimeUnit::Microseconds,
                        };
                        Ok(Field::new(field.name, DataType::Time(tu)))
                    }
                    _ => Err(DaftError::TypeError(format!(
                        "Expected input to time to be time or timestamp, got {}",
                        field.dtype
                    ))),
                },
                Err(e) => Err(e),
            },
            _ => Err(DaftError::SchemaMismatch(format!(
                "Expected 1 input arg, got {}",
                inputs.len()
            ))),
        }
    }

    fn evaluate(&self, inputs: &[Series]) -> DaftResult<Series> {
        match inputs {
            [input] => input.dt_time(),
            _ => Err(DaftError::ValueError(format!(
                "Expected 1 input arg, got {}",
                inputs.len()
            ))),
        }
    }
}

#[must_use]
pub fn dt_time(input: ExprRef) -> ExprRef {
    ScalarFunction::new(Time {}, vec![input]).into()
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use super::truncate::Truncate;

    #[test]
    fn test_fn_name() {
        use super::*;
        let cases: Vec<(Arc<dyn ScalarUDF>, &str)> = vec![
            (Arc::new(Date), "date"),
            (Arc::new(Day), "day"),
            (Arc::new(Hour), "hour"),
            (Arc::new(DayOfWeek), "day_of_week"),
            (Arc::new(DayOfYear), "day_of_year"),
            (Arc::new(Minute), "minute"),
            (Arc::new(Month), "month"),
            (Arc::new(Second), "second"),
            (Arc::new(Millisecond), "millisecond"),
            (Arc::new(Nanosecond), "nanosecond"),
            (Arc::new(Time), "time"),
            (Arc::new(Year), "year"),
            (
                Arc::new(Truncate {
                    interval: String::new(),
                }),
                "truncate",
            ),
        ];

        for (f, name) in cases {
            assert_eq!(f.name(), name);
        }
    }
}

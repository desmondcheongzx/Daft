use common_error::{DaftError, DaftResult};
use daft_core::prelude::*;
use daft_dsl::{
    functions::{ScalarFunction, ScalarUDF},
    ExprRef,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ImageResize {
    pub width: u32,
    pub height: u32,
}

#[typetag::serde]
impl ScalarUDF for ImageResize {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn name(&self) -> &'static str {
        "image_resize"
    }

    fn to_field(&self, inputs: &[ExprRef], schema: &Schema) -> DaftResult<Field> {
        match inputs {
            [input] => {
                let field = input.to_field(schema)?;

                match &field.dtype {
                    DataType::Image(mode) => match mode {
                        Some(mode) => Ok(Field::new(
                            field.name,
                            DataType::FixedShapeImage(*mode, self.height, self.width),
                        )),
                        None => Ok(field.clone()),
                    },
                    DataType::FixedShapeImage(..) => Ok(field.clone()),
                    _ => Err(DaftError::TypeError(format!(
                        "ImageResize can only resize ImageArrays and FixedShapeImageArrays, got {field}"
                    ))),
                }
            }
            _ => Err(DaftError::SchemaMismatch(format!(
                "Expected 1 input arg, got {}",
                inputs.len()
            ))),
        }
    }

    fn evaluate(&self, inputs: &[Series]) -> DaftResult<Series> {
        match inputs {
            [input] => daft_image::series::resize(input, self.width, self.height),
            _ => Err(DaftError::ValueError(format!(
                "Expected 1 input arg, got {}",
                inputs.len()
            ))),
        }
    }
}

#[must_use]
pub fn resize(input: ExprRef, w: u32, h: u32) -> ExprRef {
    ScalarFunction::new(
        ImageResize {
            width: w,
            height: h,
        },
        vec![input],
    )
    .into()
}

use std::sync::Arc;

use common_error::{DaftError, DaftResult};

use crate::{
    array::growable::{Growable, GrowableArray},
    datatypes::{DaftArrayType, DataType, Field},
    series::Series,
};

#[derive(Clone, Debug)]
pub struct StructArray {
    pub field: Arc<Field>,

    /// Column representations
    pub children: Vec<Series>,
    validity: Option<arrow2::bitmap::Bitmap>,
    len: usize,
}

impl DaftArrayType for StructArray {
    fn data_type(&self) -> &DataType {
        &self.field.as_ref().dtype
    }
}

impl StructArray {
    pub fn new<F: Into<Arc<Field>>>(
        field: F,
        children: Vec<Series>,
        validity: Option<arrow2::bitmap::Bitmap>,
    ) -> Self {
        let field: Arc<Field> = field.into();
        match &field.as_ref().dtype {
            DataType::Struct(fields) => {
                assert!(fields.len() == children.len(), "StructArray::new received {} children arrays but expected {} for specified dtype: {}", children.len(), fields.len(), &field.as_ref().dtype);
                for (dtype_field, series) in fields.iter().zip(children.iter()) {
                    assert!(!(&dtype_field.dtype != series.data_type()), "StructArray::new received an array with dtype: {} but expected child field: {}", series.data_type(), dtype_field);
                    assert!(
                        dtype_field.name == series.name(),
                        "StructArray::new received a series with name: {} but expected name: {}",
                        series.name(),
                        &dtype_field.name
                    );
                }

                let len = if !children.is_empty() {
                    children[0].len()
                } else {
                    0
                };

                for s in &children {
                    assert!(s.len() == len, "StructArray::new expects all children to have the same length, but received: {} vs {}", s.len(), len);
                }
                if let Some(some_validity) = &validity
                    && some_validity.len() != len
                {
                    panic!(
                        "StructArray::new expects validity to have length {} but received: {}",
                        len,
                        some_validity.len()
                    )
                }

                Self {
                    field,
                    children,
                    validity,
                    len,
                }
            }
            _ => panic!(
                "StructArray::new expected Struct datatype, but received field: {}",
                field
            ),
        }
    }

    pub fn validity(&self) -> Option<&arrow2::bitmap::Bitmap> {
        self.validity.as_ref()
    }

    pub fn concat(arrays: &[&Self]) -> DaftResult<Self> {
        if arrays.is_empty() {
            return Err(DaftError::ValueError(
                "Need at least 1 StructArray to concat".to_string(),
            ));
        }

        let first_array = arrays.first().unwrap();
        let mut growable = <Self as GrowableArray>::make_growable(
            first_array.field.name.as_str(),
            &first_array.field.dtype,
            arrays.to_vec(),
            arrays
                .iter()
                .map(|a| a.validity.as_ref().map_or(0usize, |v| v.unset_bits()))
                .sum::<usize>()
                > 0,
            arrays.iter().map(|a| a.len()).sum(),
        );

        for (i, arr) in arrays.iter().enumerate() {
            growable.extend(i, 0, arr.len());
        }

        growable
            .build()
            .map(|s| s.downcast::<Self>().unwrap().clone())
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn name(&self) -> &str {
        &self.field.name
    }

    pub fn data_type(&self) -> &DataType {
        &self.field.dtype
    }

    pub fn rename(&self, name: &str) -> Self {
        Self {
            field: Arc::new(Field::new(name, self.data_type().clone())),
            children: self.children.clone(),
            validity: self.validity.clone(),
            len: self.len,
        }
    }

    pub fn slice(&self, start: usize, end: usize) -> DaftResult<Self> {
        if start > end {
            return Err(DaftError::ValueError(format!(
                "Trying to slice array with negative length, start: {start} vs end: {end}"
            )));
        }
        Ok(Self::new(
            self.field.clone(),
            self.children
                .iter()
                .map(|s| s.slice(start, end))
                .collect::<DaftResult<Vec<Series>>>()?,
            self.validity
                .as_ref()
                .map(|v| v.clone().sliced(start, end - start)),
        ))
    }

    pub fn to_arrow(&self) -> Box<dyn arrow2::array::Array> {
        let arrow_dtype = self.data_type().to_arrow().unwrap();
        Box::new(arrow2::array::StructArray::new(
            arrow_dtype,
            self.children.iter().map(|s| s.to_arrow()).collect(),
            self.validity.clone(),
        ))
    }

    pub fn with_validity(&self, validity: Option<arrow2::bitmap::Bitmap>) -> DaftResult<Self> {
        if let Some(v) = &validity
            && v.len() != self.len()
        {
            return Err(DaftError::ValueError(format!(
                "validity mask length does not match StructArray length, {} vs {}",
                v.len(),
                self.len()
            )));
        }

        Ok(Self::new(
            self.field.clone(),
            self.children.clone(),
            validity,
        ))
    }
}

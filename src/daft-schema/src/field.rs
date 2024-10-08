use std::{hash::Hash, sync::Arc};

use arrow2::datatypes::Field as ArrowField;
use common_error::{DaftError, DaftResult};
use derive_more::Display;
use serde::{Deserialize, Serialize};

use crate::dtype::DataType;

pub type Metadata = std::collections::BTreeMap<String, String>;

#[derive(Clone, Display, Debug, Eq, Deserialize, Serialize)]
#[display("{name}#{dtype}")]
pub struct Field {
    pub name: String,
    pub dtype: DataType,
    pub metadata: Arc<Metadata>,
}

pub type FieldRef = Arc<Field>;
pub type DaftField = Field;

#[derive(Clone, Display, Debug, PartialEq, Eq, Deserialize, Serialize, Hash)]
#[display("{id}")]
pub struct FieldID {
    pub id: Arc<str>,
}

impl PartialEq for Field {
    fn eq(&self, other: &Self) -> bool {
        // Skips metadata check
        self.name == other.name && self.dtype == other.dtype
    }
}

impl Hash for Field {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.dtype.hash(state);
        // Skip hashing metadata
    }
}

impl FieldID {
    pub fn new<S: Into<Arc<str>>>(id: S) -> Self {
        Self { id: id.into() }
    }

    /// Create a Field ID directly from a real column name.
    /// Performs sanitization on the name so it can be composed.
    pub fn from_name<S: Into<String>>(name: S) -> Self {
        let name: String = name.into();

        // Escape parentheses within a string,
        // since we will use parentheses as delimiters in our semantic expression IDs.
        let sanitized = name
            .replace('.', "\\x2e")
            .replace(',', "\\x2c")
            .replace('(', "\\x28")
            .replace(')', "\\x29")
            .replace('\\', "\\x5c");
        Self::new(sanitized)
    }
}

impl Field {
    pub fn new<S: Into<String>>(name: S, dtype: DataType) -> Self {
        let name: String = name.into();
        Self {
            name,
            dtype,
            metadata: Default::default(),
        }
    }

    pub fn with_metadata<M: Into<Arc<Metadata>>>(self, metadata: M) -> Self {
        Self {
            name: self.name,
            dtype: self.dtype,
            metadata: metadata.into(),
        }
    }

    pub fn to_arrow(&self) -> DaftResult<ArrowField> {
        Ok(
            ArrowField::new(self.name.clone(), self.dtype.to_arrow()?, true)
                .with_metadata(self.metadata.as_ref().clone()),
        )
    }

    pub fn to_physical(&self) -> Self {
        Self {
            name: self.name.clone(),
            dtype: self.dtype.to_physical(),
            metadata: self.metadata.clone(),
        }
    }

    pub fn rename<S: Into<String>>(&self, name: S) -> Self {
        Self {
            name: name.into(),
            dtype: self.dtype.clone(),
            metadata: self.metadata.clone(),
        }
    }

    pub fn to_list_field(&self) -> DaftResult<Self> {
        if self.dtype.is_python() {
            return Ok(self.clone());
        }
        let list_dtype = DataType::List(Box::new(self.dtype.clone()));
        Ok(Self {
            name: self.name.clone(),
            dtype: list_dtype,
            metadata: self.metadata.clone(),
        })
    }

    pub fn to_fixed_size_list_field(&self, size: usize) -> DaftResult<Self> {
        if self.dtype.is_python() {
            return Ok(self.clone());
        }
        let list_dtype = DataType::FixedSizeList(Box::new(self.dtype.clone()), size);
        Ok(Self {
            name: self.name.clone(),
            dtype: list_dtype,
            metadata: self.metadata.clone(),
        })
    }

    pub fn to_exploded_field(&self) -> DaftResult<Self> {
        match &self.dtype {
            DataType::List(child_dtype) | DataType::FixedSizeList(child_dtype, _) => {
                Ok(Self {
                    name: self.name.clone(),
                    dtype: child_dtype.as_ref().clone(),
                    metadata: self.metadata.clone(),
                })
            }
            _ => Err(DaftError::ValueError(format!(
                "Column \"{}\" with dtype {} cannot be exploded, must be a List or FixedSizeList column.",
                self.name, self.dtype,
            ))),
        }
    }
}

impl From<&ArrowField> for Field {
    fn from(af: &ArrowField) -> Self {
        Self {
            name: af.name.clone(),
            dtype: af.data_type().into(),
            metadata: af.metadata.clone().into(),
        }
    }
}

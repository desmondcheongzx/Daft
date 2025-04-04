use std::sync::Arc;

use common_error::DaftResult;
use daft_core::{
    prelude::{Schema, UInt64Array, UInt8Array, Utf8Array},
    series::IntoSeries,
};
use daft_micropartition::MicroPartition;
use daft_recordbatch::RecordBatch;

use crate::{FileWriter, WriterFactory};

pub(crate) struct DummyWriterFactory;

impl WriterFactory for DummyWriterFactory {
    type Input = Arc<MicroPartition>;
    type Result = Option<RecordBatch>;

    fn create_writer(
        &self,
        file_idx: usize,
        partition_values: Option<&RecordBatch>,
    ) -> DaftResult<Box<dyn FileWriter<Input = Self::Input, Result = Self::Result>>> {
        Ok(Box::new(DummyWriter {
            file_idx: file_idx.to_string(),
            partition_values: partition_values.cloned(),
            write_count: 0,
            byte_count: 0,
        })
            as Box<
                dyn FileWriter<Input = Self::Input, Result = Self::Result>,
            >)
    }
}

pub(crate) struct DummyWriter {
    file_idx: String,
    partition_values: Option<RecordBatch>,
    write_count: usize,
    byte_count: usize,
}

impl FileWriter for DummyWriter {
    type Input = Arc<MicroPartition>;
    type Result = Option<RecordBatch>;

    fn write(&mut self, input: Self::Input) -> DaftResult<usize> {
        self.write_count += 1;
        let size_bytes = input.size_bytes()?.unwrap();
        self.byte_count += size_bytes;
        Ok(size_bytes)
    }

    fn bytes_written(&self) -> usize {
        self.byte_count
    }

    fn bytes_per_file(&self) -> Vec<usize> {
        vec![self.byte_count]
    }

    fn close(&mut self) -> DaftResult<Self::Result> {
        let path_series =
            Utf8Array::from_values("path", std::iter::once(self.file_idx.clone())).into_series();
        let write_count_series =
            UInt64Array::from_values("write_count", std::iter::once(self.write_count as u64))
                .into_series();
        let path_table = RecordBatch::new_unchecked(
            Schema::new(vec![
                path_series.field().clone(),
                write_count_series.field().clone(),
            ])
            .unwrap(),
            vec![path_series.into(), write_count_series.into()],
            1,
        );
        if let Some(partition_values) = self.partition_values.take() {
            let unioned = path_table.union(&partition_values)?;
            Ok(Some(unioned))
        } else {
            Ok(Some(path_table))
        }
    }
}

pub(crate) fn make_dummy_mp(size_bytes: usize) -> Arc<MicroPartition> {
    let series = UInt8Array::from_values("ints", std::iter::repeat_n(42, size_bytes)).into_series();
    let schema = Arc::new(Schema::new(vec![series.field().clone()]).unwrap());
    let table = RecordBatch::new_unchecked(schema.clone(), vec![series.into()], size_bytes);
    Arc::new(MicroPartition::new_loaded(
        schema.into(),
        vec![table].into(),
        None,
    ))
}

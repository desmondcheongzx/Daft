from __future__ import annotations

import pathlib

from daft.daft import (
    CsvParseOptions,
    JsonParseOptions,
    NativeStorageConfig,
    PythonStorageConfig,
    StorageConfig,
)
from daft.datatype import DataType
from daft.dependencies import pacsv, pajson, pq
from daft.filesystem import _resolve_paths_and_filesystem
from daft.logical.schema import Schema
from daft.runners.partitioning import TableParseCSVOptions
from daft.table import MicroPartition
from daft.table.table_io import FileInput, _open_stream


def from_csv(
    file: FileInput,
    storage_config: StorageConfig | None = None,
    csv_options: TableParseCSVOptions = TableParseCSVOptions(),
) -> Schema:
    """Infers a Schema from a CSV file.

    Args:
        file (str | IO): either a file-like object or a string file path (potentially prefixed with a protocol such as "s3://")
        fs (fsspec.AbstractFileSystem): fsspec FileSystem to use for reading data.
            By default, Daft will automatically construct a FileSystem instance internally.
        csv_options (vPartitionParseCSVOptions, optional): CSV-specific configs to apply when reading the file
        read_options (TableReadOptions, optional): Options for reading the file
    Returns:
        Schema: Inferred Schema from the CSV.
    """
    # Have PyArrow generate the column names if user specifies that there are no headers
    pyarrow_autogenerate_column_names = csv_options.header_index is None

    io_config = None
    if storage_config is not None:
        config = storage_config.config
        if isinstance(config, NativeStorageConfig):
            assert isinstance(file, (str, pathlib.Path)), "Native downloader only works on string inputs to read_csv"
            io_config = config.io_config
            return Schema.from_csv(
                str(file),
                parse_options=CsvParseOptions(
                    has_header=csv_options.header_index is not None,
                    delimiter=csv_options.delimiter,
                    double_quote=csv_options.double_quote,
                    quote=csv_options.quote,
                    allow_variable_columns=csv_options.allow_variable_columns,
                    escape_char=csv_options.escape_char,
                    comment=csv_options.comment,
                ),
                io_config=io_config,
            )

        assert isinstance(config, PythonStorageConfig)
        io_config = config.io_config

    with _open_stream(file, io_config) as f:
        reader = pacsv.open_csv(
            f,
            parse_options=pacsv.ParseOptions(
                delimiter=csv_options.delimiter,
            ),
            read_options=pacsv.ReadOptions(
                autogenerate_column_names=pyarrow_autogenerate_column_names,
            ),
        )

    return Schema.from_pyarrow_schema(reader.schema)


def from_json(
    file: FileInput,
    storage_config: StorageConfig | None = None,
) -> Schema:
    """Reads a Schema from a JSON file.

    Args:
        file (FileInput): either a file-like object or a string file path (potentially prefixed with a protocol such as "s3://")
        read_options (TableReadOptions, optional): Options for reading the file

    Returns:
        Schema: Inferred Schema from the JSON
    """
    io_config = None
    if storage_config is not None:
        config = storage_config.config
        if isinstance(config, NativeStorageConfig):
            assert isinstance(file, (str, pathlib.Path)), "Native downloader only works on string inputs to read_json"
            io_config = config.io_config
            return Schema.from_json(
                str(file),
                parse_options=JsonParseOptions(),
                io_config=io_config,
            )

        assert isinstance(config, PythonStorageConfig)
        io_config = config.io_config

    with _open_stream(file, io_config) as f:
        table = pajson.read_json(f)

    return MicroPartition.from_arrow(table).schema()


def from_parquet(
    file: FileInput,
    storage_config: StorageConfig | None = None,
) -> Schema:
    """Infers a Schema from a Parquet file."""
    io_config = None
    if storage_config is not None:
        config = storage_config.config
        if isinstance(config, NativeStorageConfig):
            assert isinstance(
                file, (str, pathlib.Path)
            ), "Native downloader only works on string inputs to read_parquet"
            io_config = config.io_config
            return Schema.from_parquet(str(file), io_config=io_config)

        assert isinstance(config, PythonStorageConfig)
        io_config = config.io_config

    if not isinstance(file, (str, pathlib.Path)):
        # BytesIO path.
        f = file
    else:
        paths, fs = _resolve_paths_and_filesystem(file, io_config=io_config)
        assert len(paths) == 1
        path = paths[0]
        f = fs.open_input_file(path)

    pqf = pq.ParquetFile(f)
    arrow_schema = pqf.metadata.schema.to_arrow_schema()

    return Schema._from_field_name_and_types([(f.name, DataType.from_arrow_type(f.type)) for f in arrow_schema])

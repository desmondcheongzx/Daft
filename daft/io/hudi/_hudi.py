# ruff: noqa: I002
# isort: dont-add-import: from __future__ import annotations

from typing import Optional

from daft import context
from daft.api_annotations import PublicAPI
from daft.daft import IOConfig, ScanOperatorHandle, StorageConfig
from daft.dataframe import DataFrame
from daft.logical.builder import LogicalPlanBuilder


@PublicAPI
def read_hudi(
    table_uri: str,
    io_config: Optional[IOConfig] = None,
) -> DataFrame:
    """Create a DataFrame from a Hudi table.

    Args:
        table_uri: URI to the Hudi table.
        io_config: A custom IOConfig to use when accessing Hudi table object storage data. Defaults to None.

    Returns:
        DataFrame: A DataFrame with the schema converted from the specified Hudi table.

    Examples:
        >>> df = daft.read_hudi("some-table-uri")
        >>> df = df.where(df["foo"] > 5)
        >>> df.show()
    """
    from daft.io.hudi.hudi_scan import HudiScanOperator

    io_config = context.get_context().daft_planning_config.default_io_config if io_config is None else io_config

    multithreaded_io = context.get_context().get_or_create_runner().name != "ray"
    storage_config = StorageConfig(multithreaded_io, io_config)

    hudi_operator = HudiScanOperator(table_uri, storage_config=storage_config)

    handle = ScanOperatorHandle.from_python_scan_operator(hudi_operator)
    builder = LogicalPlanBuilder.from_tabular_scan(scan_operator=handle)
    return DataFrame(builder)

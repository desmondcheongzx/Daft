from __future__ import annotations

import itertools

import pytest

from daft import utils
from daft.daft import JoinType
from daft.datatype import DataType
from daft.expressions import col
from daft.recordbatch import MicroPartition
from daft.series import Series

daft_int_types = [
    DataType.int8(),
    DataType.int16(),
    DataType.int32(),
    DataType.int64(),
    DataType.uint8(),
    DataType.uint16(),
    DataType.uint32(),
    DataType.uint64(),
]

daft_numeric_types = daft_int_types + [DataType.float32(), DataType.float64()]
daft_string_types = [DataType.string()]


def skip_null_safe_equal_for_smj(func):
    from functools import wraps
    from inspect import getfullargspec

    @wraps(func)
    def wrapper(*args, **kwargs):
        spec = getfullargspec(func)
        join_impl, null_safe_equal = None, None
        if "join_impl" in kwargs:
            join_impl = kwargs["join_impl"]
        elif "join_impl" in spec.args:
            idx = spec.args.index("join_impl")
            join_impl = args[idx] if idx >= 0 else None
        if "null_safe_equal" in kwargs:
            null_safe_equal = kwargs["null_safe_equal"]
        elif "null_safe_equal" in spec.args:
            idx = spec.args.index("null_safe_equal")
            null_safe_equal = args[idx] if idx >= 0 else None
        if join_impl == "sort_merge_join" and null_safe_equal:
            pytest.skip("sort merge join does not support null safe equal yet")
        return func(*args, **kwargs)

    return wrapper


@skip_null_safe_equal_for_smj
@pytest.mark.parametrize(
    "dtype, data, null_safe_equal",
    itertools.product(
        daft_numeric_types + daft_string_types,
        [
            (
                [0, 1, 2, 3, None],
                [0, 1, 2, 3, None],
                [(0, 0), (1, 1), (2, 2), (3, 3)],
                [(0, 0), (1, 1), (2, 2), (3, 3), (4, 4)],
            ),
            (
                [None, None, 3, 1, 2, 0],
                [0, 1, 2, 3, None],
                [(5, 0), (3, 1), (4, 2), (2, 3)],
                [(5, 0), (3, 1), (4, 2), (2, 3), (0, 4), (1, 4)],
            ),
            ([None, 4, 5, 6, 7], [0, 1, 2, 3, None], [], [(0, 4)]),
            (
                [None, 0, 0, 0, 1, None],
                [0, 1, 2, 3, None],
                [(1, 0), (2, 0), (3, 0), (4, 1)],
                [(1, 0), (2, 0), (3, 0), (4, 1), (5, 4), (0, 4)],
            ),
            (
                [None, 0, 0, 1, 1, None],
                [0, 1, 2, 3, None],
                [(1, 0), (2, 0), (3, 1), (4, 1)],
                [(1, 0), (2, 0), (3, 1), (4, 1), (5, 4), (0, 4)],
            ),
            (
                [None, 0, 0, 1, 1, None],
                [3, 1, 0, 2, None],
                [(1, 2), (2, 2), (3, 1), (4, 1)],
                [(1, 2), (2, 2), (3, 1), (4, 1), (5, 4), (0, 4)],
            ),
        ],
        [True, False],
    ),
)
@pytest.mark.parametrize("join_impl", ["hash_join", "sort_merge_join"])
def test_table_join_single_column(join_impl, dtype, data, null_safe_equal) -> None:
    left, right, expected_pairs, null_safe_expected = data
    expected_pairs = null_safe_expected if null_safe_equal else expected_pairs
    null_equals_nulls = {"null_equals_nulls": [null_safe_equal]} if null_safe_equal else {}
    left_table = MicroPartition.from_pydict({"x": left, "x_ind": list(range(len(left)))}).eval_expression_list(
        [col("x").cast(dtype), col("x_ind")]
    )
    right_table = MicroPartition.from_pydict({"y": right, "y_ind": list(range(len(right)))})
    result_table: MicroPartition = getattr(left_table, join_impl)(
        right_table, left_on=[col("x")], right_on=[col("y")], how=JoinType.Inner, **null_equals_nulls
    )

    assert result_table.column_names() == ["x", "x_ind", "y", "y_ind"]

    result_pairs = list(
        zip(result_table.get_column_by_name("x_ind").to_pylist(), result_table.get_column_by_name("y_ind").to_pylist())
    )
    assert sorted(expected_pairs) == sorted(result_pairs)
    casted_l = left_table.get_column_by_name("x").to_pylist()
    result_l = [casted_l[idx] for idx, _ in result_pairs]
    assert result_table.get_column_by_name("x").to_pylist() == result_l

    casted_r = right_table.get_column_by_name("y").to_pylist()
    result_r = [casted_r[idx] for _, idx in result_pairs]
    assert result_table.get_column_by_name("y").to_pylist() == result_r

    # make sure the result is the same with right table on left
    result_table: MicroPartition = getattr(right_table, join_impl)(
        left_table, right_on=[col("x")], left_on=[col("y")], how=JoinType.Inner, **null_equals_nulls
    )

    assert result_table.column_names() == ["y", "y_ind", "x", "x_ind"]

    result_pairs = list(
        zip(result_table.get_column_by_name("x_ind").to_pylist(), result_table.get_column_by_name("y_ind").to_pylist())
    )
    assert sorted(expected_pairs) == sorted(result_pairs)
    casted_l = left_table.get_column_by_name("x").to_pylist()
    result_l = [casted_l[idx] for idx, _ in result_pairs]
    assert result_table.get_column_by_name("x").to_pylist() == result_l

    casted_r = right_table.get_column_by_name("y").to_pylist()
    result_r = [casted_r[idx] for _, idx in result_pairs]
    assert result_table.get_column_by_name("y").to_pylist() == result_r


@pytest.mark.parametrize("join_impl", ["hash_join", "sort_merge_join"])
def test_table_join_mismatch_column(join_impl) -> None:
    left_table = MicroPartition.from_pydict({"x": [1, 2, 3, 4], "y": [2, 3, 4, 5]})
    right_table = MicroPartition.from_pydict({"a": [1, 2, 3, 4], "b": [2, 3, 4, 5]})

    with pytest.raises(ValueError, match="Mismatch of number of join keys"):
        getattr(left_table, join_impl)(right_table, left_on=[col("x"), col("y")], right_on=[col("a")])


@skip_null_safe_equal_for_smj
@pytest.mark.parametrize(
    "left",
    [
        {"a": [], "b": []},
        {"a": ["apple", "banana"], "b": [3, 4]},
    ],
)
@pytest.mark.parametrize(
    "right",
    [
        {"x": [], "y": []},
        {"x": ["banana", "apple"], "y": [3, 4]},
    ],
)
@pytest.mark.parametrize("join_impl", ["hash_join", "sort_merge_join"])
@pytest.mark.parametrize("null_safe_equal", [True, False])
def test_table_join_multicolumn_empty_result(join_impl, left, right, null_safe_equal) -> None:
    """Various multicol joins that should all produce an empty result."""
    left_table = MicroPartition.from_pydict(left).eval_expression_list(
        [col("a").cast(DataType.string()), col("b").cast(DataType.int32())]
    )
    right_table = MicroPartition.from_pydict(right).eval_expression_list(
        [col("x").cast(DataType.string()), col("y").cast(DataType.int32())]
    )

    null_equals_nulls = {"null_equals_nulls": [null_safe_equal] * 2} if null_safe_equal else {}

    result = getattr(left_table, join_impl)(
        right_table, left_on=[col("a"), col("b")], right_on=[col("x"), col("y")], **null_equals_nulls
    )
    assert result.to_pydict() == {"a": [], "b": [], "x": [], "y": []}


@pytest.mark.parametrize(
    "join_impl,null_safe_equal", [("hash_join", True), ("hash_join", False), ("sort_merge_join", False)]
)
def test_table_join_multicolumn_nocross(join_impl, null_safe_equal) -> None:
    """A multicol join that should produce two rows and no cross product results.

    Input has duplicate join values and overlapping single-column values,
    but there should only be two correct matches, both not cross.
    """
    left_table = MicroPartition.from_pydict(
        {
            "a": ["apple", "apple", "banana", "banana", "carrot", None],
            "b": [1, 2, 2, 2, 3, 3],
            "c": [1, 2, 3, 4, 5, 5],
        }
    )
    right_table = MicroPartition.from_pydict(
        {
            "x": ["banana", "carrot", "apple", "banana", "apple", "durian", None],
            "y": [1, 3, 2, 1, 3, 6, 3],
            "z": [1, 2, 3, 4, 5, 6, 6],
        }
    )

    null_equals_nulls = {"null_equals_nulls": [null_safe_equal] * 2} if null_safe_equal else {}
    result = getattr(left_table, join_impl)(
        right_table, left_on=[col("a"), col("b")], right_on=[col("x"), col("y")], **null_equals_nulls
    )
    expected = [
        {"a": "apple", "b": 2, "c": 2, "x": "apple", "y": 2, "z": 3},
        {"a": "carrot", "b": 3, "c": 5, "x": "carrot", "y": 3, "z": 2},
    ]
    if null_safe_equal:
        expected.append({"a": None, "b": 3, "c": 5, "x": None, "y": 3, "z": 6})
    assert set(utils.freeze(utils.pydict_to_rows(result.to_pydict()))) == set(utils.freeze(expected))


@pytest.mark.parametrize(
    "join_impl,null_safe_equal", [("hash_join", True), ("hash_join", False), ("sort_merge_join", False)]
)
def test_table_join_multicolumn_cross(join_impl, null_safe_equal) -> None:
    """A multicol join that should produce a cross product and a non-cross product."""
    left_table = MicroPartition.from_pydict(
        {
            "a": ["apple", "apple", "banana", "banana", "banana", None],
            "b": [1, 0, 1, 1, 1, 1],
            "c": [1, 2, 3, 4, 5, 5],
        }
    )
    right_table = MicroPartition.from_pydict(
        {
            "x": ["apple", "apple", "banana", "banana", "banana", None],
            "y": [1, 0, 1, 1, 0, 1],
            "z": [1, 2, 3, 4, 5, 5],
        }
    )

    null_equals_nulls = {"null_equals_nulls": [null_safe_equal] * 2} if null_safe_equal else {}
    result = getattr(left_table, join_impl)(
        right_table, left_on=[col("a"), col("b")], right_on=[col("x"), col("y")], **null_equals_nulls
    )
    expected = [
        {"a": "apple", "b": 1, "c": 1, "x": "apple", "y": 1, "z": 1},
        {"a": "apple", "b": 0, "c": 2, "x": "apple", "y": 0, "z": 2},
        {"a": "banana", "b": 1, "c": 3, "x": "banana", "y": 1, "z": 3},
        {"a": "banana", "b": 1, "c": 3, "x": "banana", "y": 1, "z": 4},
        {"a": "banana", "b": 1, "c": 4, "x": "banana", "y": 1, "z": 3},
        {"a": "banana", "b": 1, "c": 4, "x": "banana", "y": 1, "z": 4},
        {"a": "banana", "b": 1, "c": 5, "x": "banana", "y": 1, "z": 3},
        {"a": "banana", "b": 1, "c": 5, "x": "banana", "y": 1, "z": 4},
    ]
    if null_safe_equal:
        expected.append({"a": None, "b": 1, "c": 5, "x": None, "y": 1, "z": 5})
    assert set(utils.freeze(utils.pydict_to_rows(result.to_pydict()))) == set(utils.freeze(expected))


@pytest.mark.parametrize(
    "join_impl,null_safe_equal", [("hash_join", True), ("hash_join", False), ("sort_merge_join", False)]
)
def test_table_join_multicolumn_all_nulls(join_impl, null_safe_equal) -> None:
    left_table = MicroPartition.from_pydict(
        {
            "a": Series.from_pylist([None, None]).cast(DataType.int64()),
            "b": Series.from_pylist([None, None]).cast(DataType.string()),
            "c": [1, 2],
        }
    )
    right_table = MicroPartition.from_pydict(
        {
            "x": Series.from_pylist([None, None]).cast(DataType.int64()),
            "y": Series.from_pylist([None, None]).cast(DataType.string()),
            "z": [1, 2],
        }
    )

    null_equals_nulls = {"null_equals_nulls": [null_safe_equal] * 2} if null_safe_equal else {}
    result = getattr(left_table, join_impl)(
        right_table, left_on=[col("a"), col("b")], right_on=[col("x"), col("y")], **null_equals_nulls
    )
    expected = []
    if null_safe_equal:
        expected = [
            {"a": None, "b": None, "c": 1, "x": None, "y": None, "z": 1},
            {"a": None, "b": None, "c": 1, "x": None, "y": None, "z": 2},
            {"a": None, "b": None, "c": 2, "x": None, "y": None, "z": 1},
            {"a": None, "b": None, "c": 2, "x": None, "y": None, "z": 2},
        ]
    assert set(utils.freeze(utils.pydict_to_rows(result.to_pydict()))) == set(utils.freeze(expected))


@pytest.mark.parametrize("join_impl", ["hash_join", "sort_merge_join"])
def test_table_join_no_columns(join_impl) -> None:
    left_table = MicroPartition.from_pydict({"x": [1, 2, 3, 4], "y": [2, 3, 4, 5]})
    right_table = MicroPartition.from_pydict({"a": [1, 2, 3, 4], "b": [2, 3, 4, 5]})

    with pytest.raises(ValueError, match="No columns were passed in to join on"):
        getattr(left_table, join_impl)(right_table, left_on=[], right_on=[])


@pytest.mark.parametrize(
    "join_impl,null_safe_equal", [("hash_join", True), ("hash_join", False), ("sort_merge_join", False)]
)
def test_table_join_single_column_name_boolean(join_impl, null_safe_equal) -> None:
    left_table = MicroPartition.from_pydict({"x": [False, True, None], "y": [0, 1, 2]})
    right_table = MicroPartition.from_pydict({"x": [None, True, False, None], "right.y": [0, 1, 2, 3]})

    null_equals_nulls = {"null_equals_nulls": [null_safe_equal]} if null_safe_equal else {}
    result_table: MicroPartition = getattr(left_table, join_impl)(
        right_table, left_on=[col("x")], right_on=[col("x")], **null_equals_nulls
    )
    assert result_table.column_names() == ["x", "y", "right.y"]
    result_sorted = result_table.sort([col("x")])
    if null_safe_equal:
        assert result_sorted.get_column_by_name("y").to_pylist() == [0, 1, 2, 2]
        assert result_sorted.get_column_by_name("right.y").to_pylist() == [2, 1, 0, 3]
    else:
        assert result_sorted.get_column_by_name("y").to_pylist() == [0, 1]
        assert result_sorted.get_column_by_name("right.y").to_pylist() == [2, 1]


@pytest.mark.parametrize("join_impl", ["hash_join", "sort_merge_join"])
def test_table_join_single_column_name_null(join_impl) -> None:
    left_table = MicroPartition.from_pydict({"x": [None, None, None], "y": [0, 1, 2]})
    right_table = MicroPartition.from_pydict({"x": [None, None, None, None], "right.y": [0, 1, 2, 3]})

    result_table: MicroPartition = getattr(left_table, join_impl)(right_table, left_on=[col("x")], right_on=[col("x")])
    assert result_table.column_names() == ["x", "y", "right.y"]
    result_sorted = result_table.sort([col("x")])
    assert result_sorted.get_column_by_name("y").to_pylist() == []
    assert result_sorted.get_column_by_name("right.y").to_pylist() == []


def test_table_join_anti() -> None:
    left_table = MicroPartition.from_pydict({"x": [1, 2, 3, 4], "y": [3, 4, 5, 6]})
    right_table = MicroPartition.from_pydict({"x": [2, 3, 5]})

    result_table = left_table.hash_join(right_table, left_on=[col("x")], right_on=[col("x")], how=JoinType.Anti)
    assert result_table.column_names() == ["x", "y"]
    result_sorted = result_table.sort([col("x")])
    assert result_sorted.get_column_by_name("y").to_pylist() == [3, 6]


def test_table_join_anti_different_names() -> None:
    left_table = MicroPartition.from_pydict({"x": [1, 2, 3, 4], "y": [3, 4, 5, 6]})
    right_table = MicroPartition.from_pydict({"z": [2, 3, 5]})

    result_table = left_table.hash_join(right_table, left_on=[col("x")], right_on=[col("z")], how=JoinType.Anti)
    assert result_table.column_names() == ["x", "y"]
    result_sorted = result_table.sort([col("x")])
    assert result_sorted.get_column_by_name("y").to_pylist() == [3, 6]

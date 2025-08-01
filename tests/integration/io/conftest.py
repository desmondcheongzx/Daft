from __future__ import annotations

import contextlib
import io
import json
import os
import pathlib
import shutil
import sys
from collections.abc import Generator
from typing import TypeVar

if sys.version_info >= (3, 10):
    from typing import TypeAlias
else:
    from typing_extensions import TypeAlias

import boto3
import numpy as np
import pytest
import s3fs
from botocore.config import Config
from PIL import Image

import daft

T = TypeVar("T")

YieldFixture: TypeAlias = Generator[T, None, None]


###
# Config fixtures
###


@pytest.fixture(scope="session")
def minio_io_config() -> daft.io.IOConfig:
    return daft.io.IOConfig(
        s3=daft.io.S3Config(
            endpoint_url="http://127.0.0.1:9000",
            key_id="minioadmin",
            access_key="minioadmin",
            use_ssl=False,
        )
    )


@pytest.fixture(scope="session")
def anonymous_minio_io_config() -> daft.io.IOConfig:
    return daft.io.IOConfig(
        s3=daft.io.S3Config(
            endpoint_url="http://127.0.0.1:9000",
            use_ssl=False,
            anonymous=True,
        )
    )


@pytest.fixture(scope="session")
def aws_public_s3_config(request) -> daft.io.IOConfig:
    # Use anonymous mode to avoid having to search for credentials in the Github Runner
    # If pytest is run with `--credentials` then we set anonymous=None to go down the credentials chain
    anonymous = None if request.config.getoption("--credentials") else True

    return daft.io.IOConfig(
        s3=daft.io.S3Config(
            # NOTE: no keys or endpoints specified for an AWS public s3 bucket
            region_name="us-west-2",
            anonymous=anonymous,
        )
    )


@pytest.fixture(scope="session")
def gcs_public_config(request) -> daft.io.IOConfig:
    # Use anonymous mode to avoid having to search for credentials in the Github Runner
    # If pytest is run with `--credentials` then we set anonymous=None to go down the credentials chain
    anonymous = None if request.config.getoption("--credentials") else True
    return daft.io.IOConfig(gcs=daft.io.GCSConfig(project_id=None, anonymous=anonymous))


@pytest.fixture(scope="session")
def azure_storage_public_config() -> daft.io.IOConfig:
    return daft.io.IOConfig(
        azure=daft.io.AzureConfig(
            storage_account="dafttestdata",
            anonymous=True,
        )
    )


@pytest.fixture(scope="session")
def nginx_config() -> tuple[str, pathlib.Path]:
    """Returns the (nginx_server_url, static_files_tmpdir) as a tuple."""
    return (
        "http://127.0.0.1:8080",
        pathlib.Path("/tmp/daft-integration-testing/nginx"),
    )


@pytest.fixture(scope="session", params=["standard", "adaptive"], ids=["standard", "adaptive"])
def retry_server_s3_config(request) -> daft.io.IOConfig:
    """Returns the URL to the local retry_server fixture."""
    retry_mode = request.param
    # set a bogus region to avoid a weird aws sdk bug that causes it to use a previously set config instead of the new one.
    # once we upgrade, we can try removing the region_name param
    return daft.io.IOConfig(
        s3=daft.io.S3Config(
            endpoint_url="http://127.0.0.1:8001",
            region_name="test",
            anonymous=True,
            num_tries=10,
            retry_mode=retry_mode,
        )
    )


###
# Mounting utilities: mount data and perform cleanup at the end of each test
###


@contextlib.contextmanager
def minio_create_bucket(
    minio_io_config: daft.io.IOConfig, bucket_name: str = "my-minio-bucket"
) -> YieldFixture[list[str]]:
    """Creates a bucket in MinIO.

    Yields a s3fs FileSystem
    """
    fs = s3fs.S3FileSystem(
        key=minio_io_config.s3.key_id,
        password=minio_io_config.s3.access_key,
        client_kwargs={"endpoint_url": minio_io_config.s3.endpoint_url},
    )
    if fs.exists(bucket_name):
        fs.rm(bucket_name, recursive=True)
    fs.mkdir(bucket_name)
    try:
        yield fs
    finally:
        fs.rm(bucket_name, recursive=True)


@contextlib.contextmanager
def minio_create_public_bucket(
    minio_io_config: daft.io.IOConfig, bucket_name: str = "my-minio-public-bucket"
) -> YieldFixture[list[str]]:
    """Creates a public bucket in MinIO.

    Yields the bucket name.
    """
    # Create authenticated S3 client to set up the bucket.
    s3_client = boto3.client(
        "s3",
        endpoint_url=minio_io_config.s3.endpoint_url,
        aws_access_key_id=minio_io_config.s3.key_id,
        aws_secret_access_key=minio_io_config.s3.access_key,
        config=Config(signature_version="s3v4"),
        region_name="us-east-1",
    )

    # Create bucket if it doesn't exist.
    try:
        s3_client.create_bucket(Bucket=bucket_name)
    except s3_client.exceptions.BucketAlreadyOwnedByYou:
        pass

    # Set bucket policy for anonymous access.
    bucket_policy = {
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Principal": "*",
                "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:ListBucket"],
                "Resource": [f"arn:aws:s3:::{bucket_name}", f"arn:aws:s3:::{bucket_name}/*"],
            }
        ],
    }
    s3_client.put_bucket_policy(Bucket=bucket_name, Policy=json.dumps(bucket_policy))

    try:
        yield bucket_name
    finally:
        paginator = s3_client.get_paginator("list_objects_v2")
        pages = paginator.paginate(Bucket=bucket_name)

        for page in pages:
            if "Contents" in page:
                objects_to_delete = [{"Key": obj["Key"]} for obj in page["Contents"]]
                if objects_to_delete:
                    s3_client.delete_objects(Bucket=bucket_name, Delete={"Objects": objects_to_delete})

        s3_client.delete_bucket(Bucket=bucket_name)


@contextlib.contextmanager
def mount_data_minio(
    minio_io_config: daft.io.IOConfig, folder: pathlib.Path, bucket_name: str = "my-minio-bucket"
) -> YieldFixture[list[str]]:
    """Mounts data in `folder` into files in minio.

    Yields a list of S3 URLs
    """
    with minio_create_bucket(minio_io_config=minio_io_config, bucket_name=bucket_name) as fs:
        urls = []
        for p in folder.glob("**/*"):
            if not p.is_file():
                continue
            key = str(p.relative_to(folder))
            url = f"s3://{bucket_name}/{key}"
            fs.write_bytes(url, p.read_bytes())
            urls.append(url)

        yield urls


@contextlib.contextmanager
def mount_data_nginx(nginx_config: tuple[str, pathlib.Path], folder: pathlib.Path) -> YieldFixture[list[str]]:
    """Mounts data in `folder` into servable static files in NGINX.

    Yields a list of HTTP URLs
    """
    server_url, static_assets_tmpdir = nginx_config

    # Cleanup any old stuff in mount folder
    for item in os.listdir(static_assets_tmpdir):
        path = static_assets_tmpdir / item
        if path.is_dir():
            shutil.rmtree(path)
        else:
            os.remove(path)

    # Copy data to mount folder
    for item in os.listdir(folder):
        src = folder / item
        dest = static_assets_tmpdir / item
        if src.is_dir():
            shutil.copytree(str(src), str(dest))
        else:
            shutil.copy2(src, dest)

    try:
        yield [f"{server_url}/{p.relative_to(folder)}" for p in folder.glob("**/*") if p.is_file()]
    finally:
        for item in os.listdir(static_assets_tmpdir):
            path = static_assets_tmpdir / item
            if path.is_dir():
                shutil.rmtree(static_assets_tmpdir / item)
            else:
                os.remove(static_assets_tmpdir / item)


###
# Image data test fixtures
###


@pytest.fixture(scope="session")
def image_data() -> YieldFixture[bytes]:
    """Bytes of a small image."""
    bio = io.BytesIO()
    image = Image.fromarray(np.ones((3, 3)).astype(np.uint8))
    image.save(bio, format="JPEG")
    return bio.getvalue()


@pytest.fixture(scope="function")
def image_data_folder(image_data, tmpdir) -> YieldFixture[str]:
    """Dumps 10 small JPEG files into a tmpdir."""
    tmpdir = pathlib.Path(tmpdir)

    for i in range(10):
        fp = tmpdir / f"{i}.jpeg"
        fp.write_bytes(image_data)

    yield tmpdir


@pytest.fixture(scope="function")
def mock_http_image_urls(
    nginx_config: tuple[str, pathlib.Path], image_data_folder: pathlib.Path
) -> YieldFixture[list[str]]:
    """Uses the docker-compose Nginx server to serve HTTP image URLs.

    This fixture yields:
        list[str]: URLs of files available on the HTTP server
    """
    with mount_data_nginx(nginx_config, image_data_folder) as urls:
        yield urls


@pytest.fixture(scope="function")
def minio_image_data_fixture(minio_io_config, image_data_folder) -> YieldFixture[list[str]]:
    """Populates the minio session with some fake data and yields (S3Config, paths)."""
    with mount_data_minio(minio_io_config, image_data_folder) as urls:
        yield urls


@pytest.fixture(scope="session")
def small_images_s3_paths() -> list[str]:
    """Paths to small *.jpg files in a public S3 bucket."""
    return [f"s3://daft-public-data/test_fixtures/small_images/rickroll{i}.jpg" for i in range(6)] + [
        f"s3a://daft-public-data/test_fixtures/small_images/rickroll{i}.jpg" for i in range(6)
    ]

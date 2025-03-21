# Installation

To install Daft, run this from your terminal:

```bash
pip install -U daft
```

## Extra Dependencies

Some Daft functionality may also require other dependencies, which are specified as "extras":

To install Daft with the extra dependencies required for interacting with AWS services, such as AWS S3, run:

```bash
pip install -U daft[aws]
```

To install Daft with the extra dependencies required for running distributed Daft on top of a [Ray cluster](https://docs.ray.io/en/latest/index.html), run:

```bash
pip install -U daft[ray]
```

To install Daft with all extras, run:

```bash
pip install -U daft[all]
```

## Advanced Installation

### Installing Nightlies

If you wish to use Daft at the bleeding edge of development, you may also install the nightly build of Daft which is built every night against the `main` branch:

```bash
pip install -U daft --pre --extra-index-url https://pypi.anaconda.org/daft-nightly/simple
```

### Installing Daft from source

```bash
pip install -U https://github.com/Eventual-Inc/Daft/archive/refs/heads/main.zip
```

Please note that Daft requires the Rust toolchain in order to build from source.

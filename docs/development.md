# Development

Clone the repository and use the Rust version required by `Cargo.toml`.
Linux GUI builds need the native development libraries installed by
[the CircleCI configuration](https://github.com/pmolfese/sumaru/blob/main/.circleci/config.yml).

```{include} ../README.md
:start-after: "## Development with Cargo"
:end-before: "## Overlays"
```

Before submitting a change, run:

```sh
cargo fmt-all -- --check
cargo check-all
cargo test-all
```

The test suite includes localhost socket tests for DriveSuma. A sandbox that
blocks listening sockets will fail those tests even if the code is correct.
Tests requiring external datasets are explicitly ignored by default.

## Rust API documentation

```sh
cargo doc --no-deps --open
```

The Read the Docs site is the user guide. Rust API documentation is built
separately by Cargo.

## Contributing documentation

See [Building and publishing the docs](documentation.md). Keep examples aligned
with `sumaru --help` and the relevant subcommand help. State scientific and
mapping limitations alongside the behavior they qualify.

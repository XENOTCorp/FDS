# Contributing

Thank you for contributing to FDS.

## Build and test

All commands run from `Code/`.

```sh
cargo test --release
cargo clippy --all-targets -- -D warnings
```

Both commands must exit 0 before you submit a change.

For a host-tuned release build:

```sh
bash build/build.sh --release
```

See [Docs/wiki/build.md](Docs/wiki/build.md) for the build reference.

## Code style

- Run `cargo fmt` before you commit.
- Follow the engineering standard in
  [Docs/standard/standard.md](Docs/standard/standard.md).
- Keep the hot path allocation-free. Preallocate buffers and tables at
  startup.
- Document unsafe code. State the safety contract in the comment.
- Do not add a hash map to the hot path.

## Commit messages

Use the form `scope: summary`.

Examples:

- `engine: event-driven default, 64 KiB TCP read`
- `benchmarks: add rankings to every row`
- `docs: getting-started, system dependency step`

## Submit a change

1. Fork the repository.
2. Create a branch. Use a name that describes the change.
3. Make the change. Add tests where the change adds behavior.
4. Run `cargo test --release` and `cargo clippy --all-targets -- -D warnings`.
5. Open a pull request. Describe the change and the test results.

## License

FDS is licensed under the Apache License 2.0. See [LICENSE](LICENSE). By
submitting a pull request, you agree to license your contribution under the
same license.

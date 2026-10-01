# Contributing

Thank you for contributing to FDS.

## Build and test

All commands run from `Code/`.

```sh
cargo fmt --all --check
cargo test --workspace --locked --no-default-features
cargo test --workspace --release --locked --no-default-features
cargo clippy --workspace --all-targets --locked --no-default-features -- -D warnings
cargo test -p mol --locked -- --ignored --test-threads=1
bash build/test-build.sh
```

All checks must exit 0. With libsctp installed, also repeat the tests
and Clippy with `--all-features`. CI covers minimal and full builds.
Optional kernel/device tests may explicitly skip unavailable capabilities.

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
- Document unsafe code. Public unsafe functions need a `# Safety`
  contract; callers need a comment explaining how they meet it.
- Test ownership, cleanup, and failure behavior, not just happy paths.
- Report performance with hardware, kernel, build flags, workload, and
  repeatable commands. Do not use unsupported SOTA or production claims.
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
4. Run the checks above.
5. Open a pull request. Describe the change and the test results.

## License

FDS is licensed under the Apache License 2.0. See [LICENSE](LICENSE). By
submitting a pull request, you agree to license your contribution under the
same license.

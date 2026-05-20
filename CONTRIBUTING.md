# Contributing

Use pull requests for changes to `qpayd`, including changes that will be merged
immediately. The PR history is the audit trail used by GitHub's generated
release notes.

Before opening a PR, run:

```sh
cargo fmt -- --check
cargo test
cargo clippy --all-targets -- -D warnings
```

## Releases

Releases are created from version tags.

1. Merge the PRs intended for the release.
2. Update `Cargo.toml` if the version should change.
3. Tag the release from `master`:

```sh
git tag v0.1.0
git push origin v0.1.0
```

The `release` workflow verifies formatting, tests, clippy, and a locked release
build. If those pass, it creates a GitHub Release using `gh release create
--generate-notes` and uploads a Linux x86_64 binary tarball plus checksums.

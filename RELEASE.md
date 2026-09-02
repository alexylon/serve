# Releasing

This project uses [cargo-release](https://rust-lang.github.io/cargo-release/).
The settings are in `release.toml`.

```bash
cargo install cargo-release
```

## Making a release

Look first. Without `--execute`, nothing is changed:

```bash
cargo release patch
```

Then do it:

```bash
cargo release patch --execute
```

That one command:

1. Bumps the version in `Cargo.toml`
2. Builds and tests the packaged crate
3. Moves everything under `## [Unreleased]` in `CHANGELOG.md` into a section
   for the new version, dated today
4. Commits as `Release X.Y.Z`
5. Tags `vX.Y.Z`
6. Publishes to crates.io
7. Pushes the commit and the tag

Pushing the tag starts `.github/workflows/release.yml`, which builds binaries
for Linux (x86-64 and arm64), macOS (Intel and Apple silicon) and Windows, and
attaches them to a GitHub release.

Say `patch`, `minor` or `major` — see the note in the README about which. On
its own, `cargo release` tries to release the version already in `Cargo.toml`.

## Before you start

- Everything committed: `git status`
- The tests pass: `cargo test`
- Formatted: `cargo fmt --check`
- Signed in to crates.io: `cargo login`
- Anything worth reading about is under `## [Unreleased]` in `CHANGELOG.md`.
  Nothing writes that for you.

## Doing less than all of it

```bash
cargo release patch --execute --no-publish   # skip crates.io
cargo release patch --execute --no-push      # keep it local
```

## If it goes wrong

Before anything was pushed:

```bash
git reset --hard HEAD~1
git tag -d vX.Y.Z
```

After the tag was pushed:

```bash
git push origin :refs/tags/vX.Y.Z
```

A version on crates.io cannot be deleted. It can only be yanked, which stops
new projects from picking it up:

```bash
cargo yank --version X.Y.Z
```

## Watch out for

`cargo publish` and `cargo package` leave a copy of the crate in
`target/package/`, and a later `cargo build` can decide that copy is the
source and skip rebuilding your edits. If a change seems to have no effect,
`rm -rf target/package`.

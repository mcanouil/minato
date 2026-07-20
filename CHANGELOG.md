# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### New features

- Compare every repository a GitHub account owns against the clones on disk, reporting what is not cloned, in sync, ahead, behind, diverged, or local only, each with the reason.
- Group repositories by the directory they sit in, with `--group` to select and `--into-group` to place new clones, and `minato move --to-group` to move an existing one.
- Clone missing repositories, fetch every clone, and fast-forward those that are strictly behind and clean, each with `--dry-run` and each reporting what it deliberately left alone.
- Report how far a fork trails the repository it was forked from.
- Browse all of it interactively with `minato tui`.
- Emit machine-readable output from every command with `--json`.


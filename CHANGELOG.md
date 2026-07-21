# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Features

- feat: compare every repository a GitHub account owns against the clones on disk, reporting what is not cloned, in sync, ahead, behind, diverged, or local only, each with the reason (#6, #7).
- feat: emit machine-readable output from every command with `--json` (#7).
- feat: clone missing repositories, fetch every clone, and fast-forward those that are strictly behind and clean, each with `--dry-run` and each reporting what it deliberately left alone (#8, #9).
- feat: group repositories by the directory they sit in, with `--group` to select and `--into-group` to place new clones, and `minato move --to-group` to move an existing one (#10, #14).
- feat: browse all of it interactively with `minato tui` (#11).
- feat: report how far a fork trails the repository it was forked from (#13).
- feat: configure how long provider responses stay cached with `cache.ttl`, defaulting to fifteen minutes (#27).

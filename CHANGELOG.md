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
- feat: hide forks and clones of untracked owners by default, with `--include-forks` and `--include-external` to show them (#30).
- feat: frame local clones as a backup of remote state in the docs and output, so a repository with no clone reads as "not backed up" rather than a bare "not cloned" (#34).
- feat: sync forks with their upstream through GitHub's merge-upstream with `minato sync-fork`, fast-forward only and with `--dry-run`, leaving a diverged fork reported rather than merged (#36).
- feat: generate the command reference from the command-line definitions, gated in CI so the documented commands and flags cannot drift from the binary (#37).

### Bug Fixes

- fix: retry a transient stream or connection reset while listing, so one cancelled response no longer fails a run enumerating several accounts (#31).
- fix: report symlinked directories the scan does not follow, so a projects tree kept behind a symlink is explained rather than showing an empty result (#32).
- fix: recognise a bare repository or mirror and report it rather than silently ignoring it, since it has no working tree to compare as a clone (#33).
- fix: skip a clone with no `origin` remote when fetching rather than attempting it, so one never-published clone no longer fails the whole `fetch` (#39).

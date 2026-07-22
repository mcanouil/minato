# Changelog

All notable changes to this project will be documented in this file.

## [Unreleased]

### Changed

- change: shrink the repository listing query, halving the page size and no longer counting release downloads, so GitHub is less likely to answer an account with a 502; the download count is dropped from `minato list` as a result (#68).

### Bug Fixes

- fix: serve the last cached repositories with a warning when GitHub is briefly unavailable while listing, so a passing 502 or unreachable host no longer fails a whole run (#68).

## 0.1.0 (2026-07-22)

First public release of Minato.

### Features

- feat: compare every repository a GitHub account owns against the clones on disk, reporting each as not backed up, in sync, ahead, behind, diverged, or local only, with the reason it is in that state (#6, #7).
- feat: clone missing repositories, fetch every clone, and fast-forward those that are strictly behind and clean, each with `--dry-run` and each reporting what it deliberately left alone (#8, #9).
- feat: sync forks with their upstream through GitHub's merge-upstream with `minato sync-fork`, fast-forward only and with `--dry-run`, leaving a diverged fork reported rather than merged (#36).
- feat: report how far a fork trails the repository it was forked from (#13).
- feat: group repositories by the directory they sit in, with `--group` to select and `--into-group` to place new clones, and `minato move --to-group` to move an existing one (#10, #14).
- feat: hide forks and clones of untracked owners by default, with `--include-forks` and `--include-external` to show them (#30).
- feat: browse the whole comparison interactively with `minato tui`, cloning the highlighted repository with `c` into a group you choose or create and refreshing the row once it lands (#11, #56, #57).
- feat: narrow every command with repeatable `--owner`, `--group`, and `--state`, and emit machine-readable output with `--json` (#7).
- feat: configure how long provider responses stay cached with `cache.ttl`, defaulting to fifteen minutes (#27).
- feat: show the latest release's download count in `minato list`, surfacing metadata already fetched (#53).
- feat: install with one line, `curl -fsSL https://m.canouil.dev/minato/install.sh | bash`, with releases shipping a `SHA256SUMS` asset and build provenance the installer and `gh attestation verify` can check (#51).
- feat: generate the command reference from the command-line definitions, gated in CI so the documented commands and flags cannot drift from the binary (#37).

### Bug Fixes

- fix: retry a transient stream or connection reset while listing, so one cancelled response no longer fails a run enumerating several accounts (#31).
- fix: report a persistent server error or an unreachable host as such when listing gives up, instead of always calling it a rate limit (#49).
- fix: report symlinked directories the scan does not follow, so a projects tree kept behind a symlink is explained rather than showing an empty result (#32).
- fix: recognise a bare repository or mirror and report it rather than silently ignoring it, since it has no working tree to compare as a clone (#33).
- fix: report a clone the scan cannot read as a failure rather than dropping it, so its remote is no longer mislabelled "not backed up" (#48).
- fix: skip a clone with no `origin` remote when fetching rather than attempting it, so one never-published clone no longer fails the whole `fetch` (#39).
- fix: report unreadable roots and skipped paths from `clone`, `fetch`, and `update` too, not only `status`, so a mistyped root is never silent (#42).
- fix: escape quotes and backslashes when building the fork-comparison GraphQL query, so an unusual branch name cannot corrupt the request (#40).
- fix: reject `.` or `..` as a repository owner or name, so a crafted identity from cache or configuration cannot build a directory-escaping path (#41).
- fix: reject a `move --to-group` name that is not a plain directory (empty, `.`, `..`, or containing a path separator), so a clone cannot be moved outside its root (#46).
- fix: pass the clone destination to git as its real path rather than a lossy display string, so a non-UTF-8 path is not mangled (#45).
- fix: give each process its own cache temporary file, so concurrent runs writing the same key cannot publish a torn entry (#47).
- fix: list each account once when a login appears under both `users` and `orgs` or in a different case, so repositories are no longer fetched, reported, and cloned twice (#44).
- fix: apply `--owner` on `list` and `sync-fork`, which previously ignored it (#50).
- fix: in the TUI, clear a status message on the next key so the key hints return, refresh the table after a fetch or update, and open the screen before the first scan so it no longer appears to hang on start (#54).

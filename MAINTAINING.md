# Maintaining HW Glance

A short runbook for the one person who maintains this project.

## 1. What runs automatically

- **CI** on every push and pull request: build, clippy, tests and the bar widget model test, plus `cargo deny` (advisories, licenses, duplicate versions, sources).
- **Weekly audit** (`.github/workflows/audit.yml`, Mondays): checks the dependencies against new security advisories. A failed run emails the owner.
- **Dependabot security alerts and fix PRs**, as soon as an advisory affects the lockfile.
- **Dependabot version updates**, monthly, one PR for Cargo and one for GitHub Actions (major updates come as separate PRs).
- **Secret scanning with push protection** blocks pushes that contain tokens.
- **Private vulnerability reporting**: reports arrive under the Security tab (see `SECURITY.md`).
- **Immutable releases**: a published release cannot be edited.
- **Rulesets**: `main` cannot be force-pushed or deleted, and `v*` tags cannot be moved or deleted. A bad release is fixed by a new version, never by moving a tag.

## 2. When a Dependabot PR or alert arrives

1. Read it, and let CI run.
2. When it is green, squash and merge.
3. If the updated crate is compiled into the shipped binaries (check with `cargo tree -i <crate> -e normal,build`; dev-dependencies do not count), cut a patch release (section 4).

## 3. Monthly routine (about 30 minutes)

1. `cargo update`, then `scripts/check.sh` and `cargo deny check`. Commit `Cargo.lock` if all is well.
2. Review open issues and pull requests.
3. Merge the Dependabot PRs.
4. Release if anything user-facing changed.

## 4. Making a release

1. Set the new version in `Cargo.toml` under `[workspace.package]` (and the widget's `manifest.json`). Use SemVer: patch for fixes, minor for features. Run `cargo check` so `Cargo.lock` follows.
2. In `CHANGELOG.md`, move "Unreleased" into a new version section with the date.
3. Write `.github/release-notes/vX.Y.Z.md`.
4. Run `scripts/check.sh`, then commit.
5. `git tag -a vX.Y.Z -m "HW Glance X.Y.Z"`
6. `git push origin main vX.Y.Z`
7. Watch the Release workflow in the Actions tab.
8. Copy `pkgver` and the `sha256sums` line from the released `PKGBUILD` into `packaging/arch/PKGBUILD` and commit. Update the AUR package too, once it exists.

## 5. A private vulnerability report

1. Acknowledge it within about a week.
2. Reproduce it.
3. Fix it in the advisory's "Temporary private fork" so the fix stays private.
4. Release a patch version (section 4).
5. Publish the advisory, and credit the reporter if they want it.

## 6. Issues

- Label them (`bug`, `enhancement`; add `question` or `hardware` if useful).
- For a hardware bug, ask for `hwglanced --once --json` if it is missing.
- It is fine to close a won't-fix request politely, with a short reason. A small project cannot do everything.

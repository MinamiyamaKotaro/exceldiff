# `docs/design/` / CI Design Security Review: exceldiff-Specific Areas (2026-08-28)

*[Japanese](design-review.md)*

**What this review is**: as noted at the top of [code-review.en.md](code-review.en.md), the old `docs/security/` was a copy of a review written for xlsxparser, and never once reviewed the areas exceldiff itself adds (`diff/`, `markdown.rs`, `grid.rs`, `cli/`, `action.yml`, `release.yml`). The old documents were deleted; this review covers only the design/CI configuration of those exceldiff-specific areas. For the parser's own security design review (the 5-phase pipeline [architecture.md](../design/architecture.md) covers), see [xlsxparser's own `docs/security/`](https://github.com/MinamiyamaKotaro/xlsxparser/tree/master/docs/security), which shares that implementation.

For code-level findings (verified by actually running a crafted `.xlsx` through the real code), see [code-review.en.md](code-review.en.md) — this document focuses on how Finding 1 there fits into the broader design, plus a design-level review of `action.yml`/`release.yml` (the GitHub Actions configuration itself).

## Overall Assessment

`action.yml`/`xlsx-diff.yml`'s shell scripts consistently guard against GitHub-Actions-specific script injection (an attacker-controlled PR title, branch name, commit message, etc., interpolated directly into a `run:` block via `${{ }}` and executed as shell syntax) — no value an attacker can actually control ever appears as a direct `${{ }}` expansion inside a `run:` block; everything is either turned into a shell variable via `env:` first and then referenced quoted as `"$VAR"` (including the commit subject `diff-scope: commit` (Issue #43) added, which binds `git log`'s own output via `$(...)` into a shell variable rather than a YAML-level expansion), or passed as a typed `with:` parameter to another action. This discipline is applied consistently in the recently-added `Resolve xlsxdiff binary` step (Issue #28) too.

On the other hand, the pre-built binary download path Issue #28 implemented has a known, accepted design limitation (recorded as Finding 1, judged not to need action) — its checksum verification is fetched from the same trust boundary (the same GitHub Release) as the binary itself, so it can't defend against the release pipeline itself being compromised.

## Findings

### Finding 1 (Informational): the pre-built binary's checksum verification catches transit corruption, but not a compromised release itself

* **Severity**: Informational (an accepted, known design trade-off — no action needed)
* **Location**: `action.yml`'s "Resolve xlsxdiff binary" step, `.github/workflows/release.yml`.
* **Details**: `action.yml` fetches `https://github.com/MinamiyamaKotaro/exceldiff/releases/download/{tag}/xlsxdiff-{tag}-{target}.tar.gz` and its `SHA256SUMS` from the same GitHub Release, cross-checks them, and only then executes the binary. That check reliably catches transit corruption or an incomplete download, but if `SHA256SUMS` itself were rewritten by an attacker (e.g. one who compromised maintainer credentials or CI secrets), both the malicious binary and its "matching" checksum would be fetched consistently, and verification would pass right through.
* **Risk scenario**: if the repo's `GITHUB_TOKEN` or maintainer access were compromised, an attacker could create/overwrite a Release containing a malicious binary without ever going through `release.yml`, and every caller of this Action would download and execute it. That said, this is fundamentally the same blast radius the source-build approach already carries (a malicious commit pushed to the repo has the same effect) — it's less a new risk pre-built binary distribution introduces than the same existing trust boundary ("trust this repo's maintainer") showing up in a different shape. Genuinely closing this gap would need signing decoupled from the release itself (e.g. keyless signing via `cosign`, or a GPG key stored somewhere other than this repo's own GitHub Releases) — judged disproportionate for the current threat model (a project run by one trusted maintainer).
* **Disposition**: no action taken for now; recorded for the record. Worth revisiting if external contributors grow or supply-chain-attack concerns increase.

## What Held Up Well

* **Consistent defense against GitHub Actions script injection**: every `run:` block in `action.yml`/`xlsx-diff.yml` was checked, and none directly expand an attacker-controllable value (a PR's branch name, title, commit message, etc.) via `${{ }}` into shell script text. Even `BASE_SHA`/`HEAD_SHA` (commit SHAs — not strings an attacker gets to freely choose) go through `env:` first and get referenced quoted — the standard defensive pattern against this well-known class of GitHub Actions vulnerability is applied consistently throughout.
* **`diff-scope: commit`'s commit-subject interpolation is doubly safe**: Issue #43's `echo "## Commit \`${short}\` — ${subject}"` binds `$subject` from `git log`'s output via `$(...)` into a shell variable — since that's shell variable expansion rather than YAML-level expansion, it's outside the scope of GitHub-Actions-specific script injection in the first place (a shell variable's expanded content is never re-parsed as new shell syntax). It's also already hardened against corrupting the Markdown comment via an unbalanced closing backtick (backtick-escaping added during implementation, PR #54).
* **`release.yml`'s packaging steps have no dangerous operations**: `tar -czf`/`sha256sum`/`gh release create` only ever use fixed arguments, `${{ github.ref_name }}` (chosen by whoever pushed the tag — not a PR-attacker-controlled context), or `matrix.target` (a fixed value enumerated in the workflow definition itself) — no attacker-controlled value is involved anywhere.
* **`xlsxdiff`'s dependency graph has no `unsafe` native extensions or C toolchain requirement at all** (confirmed via `cargo tree` during Issue #28's verification — even `zip`'s `deflate` feature is pure Rust via `flate2` → `zlib-rs`), so cross-compiling the pre-built binaries stays a simple setup that never needs `cross`/Docker — the build pipeline's own attack surface is correspondingly small.

## Out of Scope

* Design review of the parser proper (`container/`, `parse/`, `model/`, `resolve/`, `json.rs`, `pipeline.rs`, `error.rs`, `lib.rs`) — see xlsxparser's own `docs/security/`.
* Supply-chain/dependency vulnerabilities in `quick-xml`, `zip`, `serde`/`serde_json`, `thiserror`, `rusqlite`, `dtolnay/rust-toolchain`, `Swatinem/rust-cache`, `peter-evans/*`, or any `actions/*` Marketplace Action — routine version-pinning/Dependabot-style operational concerns, not a project-specific design decision, so out of scope here.
* Code-level verification (actually running a crafted file through the real code) — see [code-review.en.md](code-review.en.md); this document is scoped to design/CI-configuration-level concerns only.

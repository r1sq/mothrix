# Updates

The release files from GitHub (the Linux binary and the Windows `.exe`)
look for a newer release 10 seconds after starting and then once a day,
through the proxy setting. The settings (`F2`, **Updates**, or
`updates = false` in `config.toml`) turn that off. When there is one, the
status line says so once. Installing always takes asking: `/update` (or
`mothrix update` in a shell) downloads it, checks it and puts it in
place. On Linux mothrix then starts again by itself, in the same terminal,
with the same arguments. On Windows it says the new one is installed and
ends: start it again.

- A **package** (the `.deb`, the PKGBUILD) only says that a release is
  out; your package manager updates it. That too takes a build at a
  release's tag: the PKGBUILD (like `just deb`) builds the checkout it
  lies in, and off a tag its mothrix is not a release and never looks.
- A build that is **not a release** (its version names a commit, as
  `0.2.0 (ce613f6)`) never looks.
- mothrix can only replace itself in a folder you can write to. Elsewhere
  it says so and leaves things as they are.
- The new program is started once (`--version`) before it takes the old
  one's place: one that does not start on this system is not installed.
- A build without git at hand (from a source archive) counts as not a
  release.

Looking means asking GitHub about the latest release, so GitHub sees the
address mothrix runs from, once a day.

## Why an update can be trusted

A self-updater runs what it downloads, so each release has a manifest,
`release.json`, listing its version and every file's SHA-256, signed with
Ed25519 (`release.json.sig`). The public key is built into mothrix
(`crates/mothrix-core/src/release_key.pub`). A changed file, another key,
or an old release published under a new tag are all refused. A build
without a key never looks.

The signing key lives in GitHub as a secret of the `release` environment,
and that environment needs a reviewer's approval before anything is
published: someone with admin access to the repository could still sign
a release, but not without it being noticed.

To check a download by hand, compare its SHA-256 with `SHA256SUMS` or
`release.json` in the release.

## For maintainers

- `cargo run -p mothrix-release -- keygen FILE` makes the key once: the
  private half into `FILE` (never printed), the public half into the
  tree. The private half goes into the secret `RELEASE_SIGNING_KEY` of the
  GitHub environment `release`.
- The approval, and who can reach the key, are settings on GitHub that
  nothing in the repository enforces. As they were on 2026-11-24 (to be
  looked at again after any change of who has access; `gh api
  repos/r1sq/mothrix/environments/release`, `…/rulesets`,
  `…/actions/permissions/workflow` show them):
  - [x] Environment `release`: **required reviewers** set.
  - [x] Environment `release`: **deployment branches and tags** limited
        to tags `v*` (else a pushed branch with a workflow of its own
        that names the environment could read the key).
  - [x] A **tag ruleset** for `v*` ("release tags"): such a tag cannot
        be moved or deleted (a tag moved between build and approval
        would be published with the earlier commit's files). Who can
        push can still create one.
  - [x] Actions' default token permissions: read only.
- `keygen` does not replace a public key that is in the tree: installed
  copies trust that one, and there is no way yet to move them to another.
- A release is a pushed tag `vX.Y.Z` naming `Cargo.toml`'s version;
  `.github/workflows/release.yml` builds without secrets, then signs
  (`mothrix-release sign`, which refuses a key that does not match the
  one in the tree) and publishes once the environment is approved.

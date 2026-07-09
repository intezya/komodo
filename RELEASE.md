# Komodo Release Cycle

This document describes the release process that is visible in this repository. It separates confirmed automation from recommended release-operator checklist items. If a step is not backed by a workflow, script, manifest, or existing doc in this checkout, it is called out as a recommendation or an open decision.

## Release Surfaces

Komodo currently releases three runtime components:

- `komodo_core`, built as the `core` binary and the `ghcr.io/intezya/komodo-core` image.
- `komodo_periphery`, built as the `periphery` binary and the `ghcr.io/intezya/komodo-periphery` image.
- `komodo_cli`, built as the `km` binary and the `ghcr.io/intezya/komodo-cli` image.

The release workflow also publishes GitHub Release binary assets for Linux `amd64` and `arm64`.

The Rust workspace version lives in [`Cargo.toml`](Cargo.toml), under `[workspace.package]`. The binary crates use `version.workspace = true`, and Core, Periphery, and CLI expose that package version at runtime through `env!("CARGO_PKG_VERSION")` in their code. The TypeScript client has its own version in [`client/core/ts/package.json`](client/core/ts/package.json).

## Confirmed Automation

### CI

[`.github/workflows/ci.yml`](.github/workflows/ci.yml) runs on pushes and pull requests targeting `main`.

It has two jobs:

- `Build & Test`: installs stable Rust, then runs `cargo build --verbose` and `cargo test --verbose`.
- `Format`: installs stable Rust with `rustfmt`, then runs `cargo fmt --all -- --check`.

This is the repository-confirmed baseline before release. The release workflow itself does not repeat these Rust checks.

### Release Trigger

[`.github/workflows/release.yml`](.github/workflows/release.yml) runs on pushed tags matching:

```yaml
v*
```

The workflow sets `VERSION` from `GITHUB_REF_NAME`, so a pushed tag such as `v2.2.0` becomes the release version used for GitHub Release creation and Docker image tags.

### Binary Assets

The release workflow builds Linux binaries with Docker Buildx using [`bin/binaries.Dockerfile`](bin/binaries.Dockerfile):

- `linux/amd64` output goes to `dist/linux-amd64`.
- `linux/arm64` output goes to `dist/linux-arm64`.

It then prepares six executable assets:

- `km-x86_64`
- `periphery-x86_64`
- `core-x86_64`
- `km-aarch64`
- `periphery-aarch64`
- `core-aarch64`

The workflow creates a GitHub Release in `intezya/komodo` with those files and static release notes:

```sh
gh release create "$VERSION" dist/assets/* \
  --repo "$REPOSITORY" \
  --title "$VERSION" \
  --notes "Intezya Komodo release $VERSION"
```

There is no release-note generation or changelog extraction in the workflow.

### Container Images

The release workflow logs in to GHCR and publishes multi-platform `linux/amd64,linux/arm64` images using Docker Buildx:

| Component | Dockerfile | Published tags |
| --- | --- | --- |
| Core | [`bin/core/aio.Dockerfile`](bin/core/aio.Dockerfile) | `ghcr.io/intezya/komodo-core:${GITHUB_REF_NAME}`, `ghcr.io/intezya/komodo-core:2` |
| Periphery | [`bin/periphery/aio.Dockerfile`](bin/periphery/aio.Dockerfile) | `ghcr.io/intezya/komodo-periphery:${GITHUB_REF_NAME}`, `ghcr.io/intezya/komodo-periphery:2` |
| CLI | [`bin/cli/aio.Dockerfile`](bin/cli/aio.Dockerfile) | `ghcr.io/intezya/komodo-cli:${GITHUB_REF_NAME}`, `ghcr.io/intezya/komodo-cli:2` |

The moving major tag is hard-coded as `2`. For a future major release, this workflow must be updated intentionally.

### Installers and Compose Defaults

[`scripts/install-cli.py`](scripts/install-cli.py) and [`scripts/setup-periphery.py`](scripts/setup-periphery.py) default to the latest GitHub Release tag from:

```text
https://api.github.com/repos/intezya/komodo/releases/latest
```

They download binaries from:

```text
https://github.com/intezya/komodo/releases/download
```

Both scripts accept `--version/-v` to pin a release tag such as `v2.0.0`. The Periphery installer doc in [`scripts/readme.md`](scripts/readme.md) says the script can be rerun after a Komodo version release to update the Periphery binary without rewriting existing config after the first run.

The compose examples in [`compose/mongo.compose.yaml`](compose/mongo.compose.yaml) and [`compose/ferretdb.compose.yaml`](compose/ferretdb.compose.yaml) use:

```yaml
ghcr.io/intezya/komodo-core:${COMPOSE_KOMODO_IMAGE_TAG:-2}
ghcr.io/intezya/komodo-periphery:${COMPOSE_KOMODO_IMAGE_TAG:-2}
```

[`compose/compose.env`](compose/compose.env) sets:

```env
COMPOSE_KOMODO_IMAGE_TAG="2"
```

That matches the release workflow's hard-coded moving major tag.

### Release Target Check

[`scripts/check-release-targets.sh`](scripts/check-release-targets.sh) validates part of the fork-specific release target configuration:

- `.github/workflows/release.yml` must exist.
- The release workflow must reference `intezya`.
- Selected release-critical files must not still reference `moghtech`.

This script is useful as a preflight check for the current fork, but it is not invoked by CI or the release workflow in this checkout.

## Recommended Release Checklist

These steps are the safest cycle implied by the repository. Items marked "recommended" are not currently enforced by release automation.

### 1. Choose the Version

Use a tag in the form:

```text
vX.Y.Z
```

The repository uses SemVer-style examples in installer help text and release docs. The release workflow accepts any tag starting with `v`, but the installers and docs show versions like `v2.0.0`.

Recommended version files to check before tagging:

- [`Cargo.toml`](Cargo.toml): `[workspace.package].version`.
- [`client/core/ts/package.json`](client/core/ts/package.json): TypeScript client package version.
- [`compose/compose.env`](compose/compose.env): major image tag default when doing a major release.
- [`.github/workflows/release.yml`](.github/workflows/release.yml): hard-coded moving Docker tag, currently `:2`.

There is no repository-confirmed version bump script, so version changes are manual.

### 2. Prepare Release Notes

The release workflow currently creates static GitHub Release notes. If the release needs user-facing notes, prepare them before pushing the tag.

Recommended places to update when applicable:

- Add or update a release note under [`docsite/docs/releases/`](docsite/docs/releases/). The existing `v2.0.0` document is the current example.
- Update [`roadmap.md`](roadmap.md) only when the release changes roadmap status. The roadmap explicitly says specific versions are not final.
- Update setup or migration docs if the release changes image tags, config, install commands, or upgrade behavior.

### 3. Run Preflight Checks

Run the checks that CI enforces:

```sh
cargo fmt --all -- --check
cargo build --verbose
cargo test --verbose
```

Run the fork-specific release target check:

```sh
sh scripts/check-release-targets.sh
```

Recommended UI/client checks before a release that touches UI, API types, or TypeScript client output:

```sh
cd client/core/ts
yarn
yarn build

cd ../../../ui
yarn
yarn build
```

The Core image Dockerfile runs the TypeScript client build and UI build during image construction, but running them before tagging catches failures earlier.

### 4. Tag and Push

After `main` is green and the version changes are merged:

```sh
git tag vX.Y.Z
git push origin vX.Y.Z
```

Pushing the tag starts the release workflow. Do not push a tag until the commit is the intended release commit; the workflow publishes public assets and images.

### 5. Verify the Published Release

After the workflow completes, verify:

- The GitHub Release exists in `intezya/komodo` with the expected `vX.Y.Z` title.
- All six binary assets are present and executable after download.
- GHCR contains the three image families with both the exact tag and the moving major tag:
  - `ghcr.io/intezya/komodo-core:vX.Y.Z`
  - `ghcr.io/intezya/komodo-core:2`
  - `ghcr.io/intezya/komodo-periphery:vX.Y.Z`
  - `ghcr.io/intezya/komodo-periphery:2`
  - `ghcr.io/intezya/komodo-cli:vX.Y.Z`
  - `ghcr.io/intezya/komodo-cli:2`
- `scripts/install-cli.py --version vX.Y.Z` resolves the expected `km` asset for the host architecture.
- `scripts/setup-periphery.py --version vX.Y.Z` resolves the expected `periphery` asset for the host architecture.
- Compose users can pull the configured tag from the examples.

## Optional or Manual Publishing

The repo contains evidence that client packages exist, but the GitHub release workflow does not publish them.

- The TypeScript client package is defined in [`client/core/ts/package.json`](client/core/ts/package.json), and [`client/core/ts/runfile.toml`](client/core/ts/runfile.toml) has a `publish-ts-client` command that runs `npm publish`.
- The docs mention the Rust client on crates.io and the TypeScript client on npm, but this checkout does not include a release workflow for `cargo publish` or `npm publish`.

Treat npm and crates.io publishing as manual follow-up unless a separate automation path is added. If publishing either package, verify package versions match the release version before publishing.

## Known Gaps and Operator Notes

- No `CHANGELOG.md` exists in this checkout.
- GitHub Release notes are static in `.github/workflows/release.yml`.
- Version bumping is manual; no single command updates Rust, TypeScript, docs, and moving Docker tags together.
- The moving Docker tag is hard-coded as `2` in `.github/workflows/release.yml` and compose defaults. A future `v3` release must update these deliberately.
- `scripts/check-release-targets.sh` covers selected release-critical files, not every doc or legacy Dockerfile. Some non-release-critical docs and Dockerfiles may still mention upstream `moghtech` paths.
- `.github/workflows/release.yml` uses the `aio.Dockerfile` files and `bin/binaries.Dockerfile`; it does not build the older `single-arch.Dockerfile` or `multi-arch.Dockerfile` files.
- The release workflow does not run `scripts/check-release-targets.sh`; run it manually or wire it into CI before relying on it.

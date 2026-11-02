# Releasing Marshal

## First release

After the release workflow is merged into `main`:

1. Open **Actions → Release → Run workflow** on GitHub.
2. Select `main`. Leave **Publish immediately** unchecked to review a draft, or check it to publish when the build finishes.
3. Click **Run workflow**. The current version is `0.1.1`, so this creates `v0.1.1`. The existing `v0.1.0` tag is preserved.
4. Open the release linked in the workflow summary. For a draft, review the generated notes and click **Publish release**.

The workflow uses the built-in `GITHUB_TOKEN`. No PAT, signing key, or repository secret is required.

## Later releases

Update these files in a PR and merge it before running Release:

- `Cargo.toml`: package version.
- `Cargo.lock`: regenerate with Cargo so the `marshal` package version matches.
- `meson.build`: project version.
- `data/io.github._6E6B.marshal.metainfo.xml`: add a release as the first entry, with the version, date, and changes.

If dependencies changed, also run `flatpak/update-cargo-sources.sh` and commit the updated sources.

Versions use `major.minor.patch`, optionally with a prerelease suffix such as `0.2.0-rc.1`. Prerelease versions are automatically marked as GitHub prereleases. Build metadata (`+...`) is not supported.

You can also push a tag matching the version, for example `git tag v0.2.0 && git push origin v0.2.0`. Tag pushes build a draft for review. A tag pointing at mismatched versions fails before building.

## What it builds

Every release runs the same Rustfmt, Clippy, Rust tests, metadata checks, and Flatpak build as PR CI. The actual Flatpak produced by that run is attached to the release, together with:

- `marshal-VERSION-linux-x86_64.flatpak`: installable bundle with GNOME runtime information.
- `marshal-VERSION-source.tar.gz`: source from the exact tested commit, including the Cargo lockfile and Flatpak source manifest.
- `SHA256SUMS`: checksums for both downloads. Run `sha256sum --check SHA256SUMS` with both files in the same directory.

Only the final upload job has write access. It creates a draft first and uploads all assets before optional publication, so failed builds cannot publish an incomplete release. Rerunning a failed run can replace assets on its draft for the same commit. Published releases are never overwritten: use a new version. If a draft targets the wrong commit, review and delete it before rerunning.

The Flatpak targets Linux x86_64. It uses Flathub for the GNOME runtime but does not submit Marshal to Flathub, provide automatic app updates, or build Windows/macOS packages. The SHA256 file detects damaged downloads; the bundle is not GPG signed.

# Arch Linux / AUR packaging

This directory contains the AUR packaging files for the stable `tiri` package.

The package builds from the GitHub release source archive and the matching
vendored Rust dependency archive. Both files are produced by the repository's
`Prepare release` GitHub Actions workflow.

## Release update

1. Publish a GitHub release with these assets:
   - `tiri-$pkgver.tar.gz`
   - `tiri-$pkgver-vendored-dependencies.tar.xz`
   - release tag: `tiri-v$pkgver`
2. Update `pkgver` and reset `pkgrel=1` in `PKGBUILD`.
3. On Arch, refresh checksums and `.SRCINFO`:

   ```sh
   updpkgsums
   makepkg --printsrcinfo > .SRCINFO
   ```

4. Test the package:

   ```sh
   makepkg -Cfsri
   ```

5. Publish to AUR:

   ```sh
   git clone ssh://aur@aur.archlinux.org/tiri.git aur-tiri
   cp PKGBUILD .SRCINFO aur-tiri/
   cd aur-tiri
   git add PKGBUILD .SRCINFO
   git commit -m "Update to $pkgver"
   git push
   ```

Do not leave `SKIP` checksums in the AUR repository. They are placeholders so
this in-tree template can exist before the release artifacts are created.

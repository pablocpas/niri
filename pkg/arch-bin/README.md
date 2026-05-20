# Arch Linux / AUR binary packaging

This directory contains the AUR packaging files for the prebuilt `tiri-bin`
package.

The package installs a binary tarball built inside an Arch Linux container by
the repository's `Prepare release` GitHub Actions workflow:

- `tiri-$pkgver-x86_64-archlinux.tar.zst`
- release tag: `tiri-v$pkgver`

Use `tiri` for a source build and `tiri-bin` for a faster install.

## Release update

1. Publish a GitHub release with the Arch binary archive.
2. Update `pkgver` and reset `pkgrel=1` in `PKGBUILD`.
3. On Arch, refresh checksums and `.SRCINFO`:

   ```sh
   updpkgsums
   makepkg --printsrcinfo > .SRCINFO
   ```

4. Publish to AUR:

   ```sh
   git clone ssh://aur@aur.archlinux.org/tiri-bin.git aur-tiri-bin
   cp PKGBUILD .SRCINFO aur-tiri-bin/
   cd aur-tiri-bin
   git add PKGBUILD .SRCINFO
   git commit -m "Update to $pkgver"
   git push
   ```

Do not leave `SKIP` checksums in the AUR repository.

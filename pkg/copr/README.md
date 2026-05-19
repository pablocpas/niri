# COPR packaging

The Fedora spec lives in `pkg/fedora/tiri.spec`.

It is intended to build from GitHub release assets:

- `tiri-$version.tar.gz`
- `tiri-$version-vendored-dependencies.tar.xz`

Those assets are produced by the repository's `Prepare release` GitHub Actions
workflow. The vendored archive makes the RPM build independent from crates.io
and git network access during `%build`.

## First-time setup

Create the COPR project once. Pick the Fedora chroots that are current when you
publish:

```sh
copr-cli create tiri \
  --description "A tiling Wayland compositor derived from niri" \
  --instructions "sudo dnf copr enable pablocpas/tiri && sudo dnf install tiri" \
  --chroot fedora-rawhide-x86_64
```

Add additional chroots in the COPR web UI or with `copr-cli edit-chroot`.

## Build a release

After publishing the GitHub release assets:

```sh
spectool -g pkg/fedora/tiri.spec
copr-cli build tiri pkg/fedora/tiri.spec
```

If you bump `Version:` in the spec, reset `Release:` to `1%{?dist}`. Increase
only `Release:` for packaging-only fixes that use the same upstream release.

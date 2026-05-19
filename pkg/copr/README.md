# COPR packaging

The Fedora spec lives in `pkg/fedora/tiri.spec`.

It is intended to build from GitHub release assets:

- `tiri-$version.tar.gz`
- `tiri-$version-vendored-dependencies.tar.xz`

Those assets are produced by the repository's `Prepare release` GitHub Actions
workflow. The vendored archive makes the RPM build independent from crates.io
and git network access during `%build`.

Release tags intentionally use `tiri-v$version` rather than `v$version`, because
this fork still has historical upstream niri tags.

## First-time setup

Create the COPR project once. Install your API token first from
<https://copr.fedorainfracloud.org/api/> into `~/.config/copr`.

Pick the Fedora chroots that are current when you publish. As of this package's
first release, COPR exposes Fedora 42, Fedora 43, Fedora 44, and rawhide for
x86_64:

```sh
copr-cli create tiri \
  --description "A tiling Wayland compositor derived from niri" \
  --instructions "sudo dnf copr enable pablocpas/tiri && sudo dnf install tiri" \
  --chroot fedora-42-x86_64 \
  --chroot fedora-43-x86_64 \
  --chroot fedora-44-x86_64 \
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

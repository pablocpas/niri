Name:           tiri
Version:        0.1.1
Release:        1%{?dist}
Summary:        A tiling Wayland compositor

License:        GPL-3.0-or-later
URL:            https://github.com/pablocpas/tiri
Source0:        %{url}/releases/download/%{name}-v%{version}/%{name}-%{version}.tar.gz
Source1:        %{url}/releases/download/%{name}-v%{version}/%{name}-%{version}-vendored-dependencies.tar.xz
Provides:       wayland-compositor

BuildRequires:  cargo
BuildRequires:  clang
BuildRequires:  gcc
BuildRequires:  pkgconfig
BuildRequires:  cairo-gobject-devel
BuildRequires:  dbus-devel
BuildRequires:  libdisplay-info-devel
BuildRequires:  libgbm-devel
BuildRequires:  libinput-devel
BuildRequires:  libseat-devel
BuildRequires:  libudev-devel
BuildRequires:  libxkbcommon-devel
BuildRequires:  mesa-libEGL-devel
BuildRequires:  pango-devel
BuildRequires:  pipewire-devel
BuildRequires:  systemd-devel
BuildRequires:  wayland-devel

Requires:       libwayland-server
Requires:       bash
Recommends:     alacritty
Recommends:     fuzzel
Recommends:     gnome-keyring
Recommends:     mako
Recommends:     mesa-dri-drivers
Recommends:     mesa-libEGL
Recommends:     swaybg
Recommends:     swaylock
Recommends:     waybar
Recommends:     xdg-desktop-portal-gnome
Recommends:     xdg-desktop-portal-gtk
Recommends:     xwayland-satellite

%description
tiri is a tiling Wayland compositor derived from niri.

This spec is intended for COPR builds from GitHub release assets:
- Source0 is the release source tarball.
- Source1 is the vendored Rust dependencies archive published alongside the release.

%prep
%autosetup -n %{name}-%{version}
tar -xJf %{SOURCE1}

%build
export TIRI_BUILD_VERSION_STRING="%{version}"
cargo build --release --frozen

%install
install -Dpm0755 target/release/tiri %{buildroot}%{_bindir}/tiri
install -Dpm0755 resources/tiri-session %{buildroot}%{_bindir}/tiri-session
install -Dpm0644 resources/tiri.desktop %{buildroot}%{_datadir}/wayland-sessions/tiri.desktop
install -Dpm0644 resources/tiri-portals.conf %{buildroot}%{_datadir}/xdg-desktop-portal/tiri-portals.conf
install -Dpm0644 resources/profiles/i3.kdl %{buildroot}%{_datadir}/tiri/profiles/i3.kdl
install -Dpm0644 resources/tiri.service %{buildroot}%{_userunitdir}/tiri.service
install -Dpm0644 resources/tiri-shutdown.target %{buildroot}%{_userunitdir}/tiri-shutdown.target

%files
%license LICENSE
%doc README.md
%{_bindir}/tiri
%{_bindir}/tiri-session
%{_datadir}/wayland-sessions/tiri.desktop
%{_datadir}/xdg-desktop-portal/tiri-portals.conf
%{_datadir}/tiri/profiles/i3.kdl
%{_userunitdir}/tiri.service
%{_userunitdir}/tiri-shutdown.target

%changelog
* Sun Jun 07 2026 Pablo Pascual <pablocpascual@gmail.com> - 0.1.1-1
- Release 0.1.1

* Tue May 19 2026 Pablo Pascual <pablocpascual@gmail.com> - 0.1.0-2
- Use vendored cargo config for git dependencies in COPR builds

* Mon Mar 30 2026 Pablo Pascual <pablocpascual@gmail.com> - 0.1.0-1
- Initial COPR packaging template for tiri

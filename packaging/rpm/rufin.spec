Name:           rufin
Version:        0.9.0
Release:        1%{?dist}
Summary:        Native GTK4/libadwaita music client written in Rust

License:        GPL-3.0-or-later AND Apache-2.0 AND BSD-3-Clause AND CC0-1.0 AND CDLA-Permissive-2.0 AND ISC AND MIT AND MPL-2.0 AND Unicode-3.0 AND Unlicense AND Zlib
URL:            https://github.com/screwys/Rufin
Source0:        Rufin-%{version}.tar.xz
Source1:        Rufin-%{version}-vendor.tar.xz

ExclusiveArch:  x86_64 aarch64

BuildRequires:  appstream
BuildRequires:  cargo-rpm-macros
BuildRequires:  cmake
BuildRequires:  desktop-file-utils
BuildRequires:  gcc
BuildRequires:  gettext
BuildRequires:  make
BuildRequires:  perl-interpreter
BuildRequires:  pkgconfig(gdk-pixbuf-2.0)
BuildRequires:  pkgconfig(glib-2.0)
BuildRequires:  pkgconfig(gstreamer-1.0)
BuildRequires:  pkgconfig(gstreamer-audio-1.0)
BuildRequires:  pkgconfig(gtk4)
BuildRequires:  pkgconfig(libadwaita-1)
BuildRequires:  rust >= 1.95.0

Requires:       hicolor-icon-theme
Recommends:     gstreamer1-plugins-base
Recommends:     gstreamer1-plugins-good
Recommends:     gstreamer1-plugins-bad-free
Recommends:     gstreamer1-plugins-ugly-free

%description
Rufin is a native GTK4/libadwaita music client for Jellyfin, Subsonic,
Navidrome, and local music libraries.

%prep
%autosetup -n Rufin-%{version} -a1
%cargo_prep -v vendor

%build
# aws-lc-sys omits CFLAGS from one compiler probe, so pair Fedora's hardened
# linker flags with the PIE compile flag that probe still needs.
LDFLAGS="%{build_ldflags} -fPIE" \
  %{__cargo} build %{__cargo_common_opts} --profile rpm --package rufin
CARGO_HOME=.cargo RUSTC_BOOTSTRAP=1 cargo tree \
  -Zavoid-dev-deps \
  --package rufin \
  --offline \
  --edges=no-build,no-dev,no-proc-macro \
  --target=all \
  --prefix=none \
  --format='{l}: {p}' > LICENSE.dependencies
%cargo_vendor_manifest

%install
install -Dpm0755 target/rpm/rufin %{buildroot}%{_bindir}/rufin
desktop-file-install \
  --dir=%{buildroot}%{_datadir}/applications \
  data/io.github.screwys.Rufin.desktop
install -Dpm0644 data/io.github.screwys.Rufin.metainfo.xml \
  %{buildroot}%{_metainfodir}/io.github.screwys.Rufin.metainfo.xml

install -d %{buildroot}%{_datadir}/icons/hicolor
cp -a data/icons/hicolor/. %{buildroot}%{_datadir}/icons/hicolor/

for po_file in crates/localization/locales/*.po; do
  lang=${po_file##*/}
  lang=${lang%.po}
  install -d %{buildroot}%{_datadir}/locale/${lang}/LC_MESSAGES
  msgfmt "${po_file}" \
    -o %{buildroot}%{_datadir}/locale/${lang}/LC_MESSAGES/rufin.mo
done

%find_lang rufin

%check
desktop-file-validate data/io.github.screwys.Rufin.desktop
appstreamcli validate --no-net data/io.github.screwys.Rufin.metainfo.xml

%files -f rufin.lang
%license LICENSE
%license LICENSE.dependencies
%license cargo-vendor.txt
%doc README.md
%{_bindir}/rufin
%{_datadir}/applications/io.github.screwys.Rufin.desktop
%{_metainfodir}/io.github.screwys.Rufin.metainfo.xml
%{_datadir}/icons/hicolor/512x512/apps/*.png
%{_datadir}/icons/hicolor/64x64/apps/*.png
%{_datadir}/icons/hicolor/scalable/actions/*.svg
%{_datadir}/icons/hicolor/scalable/apps/*.svg
%{_datadir}/icons/hicolor/scalable/status/*.svg

%changelog
* Fri Jul 17 2026 screwy <screwygit@proton.me> - 0.9.0-1
- Initial RPM package

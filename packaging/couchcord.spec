# RPM spec for couchcord. Built in COPR from a local SRPM produced by
# packaging/build-srpm.sh (source tarball from the git tag + vendored cargo
# deps as Source1 — no rust-*-devel packages needed).
# The test suite runs by default (58 tests across the workspace, no hardware
# needed). Disable for a one-off build with --without check; COPR builds run
# the suite.
%bcond_without check

Name:           couchcord
Version:        0.1.0
Release:        1%{?dist}
Summary:        Controller-driven Discord voice control + activity overlay for gamescope game mode
License:        MIT
URL:            https://github.com/MasonRhodesDev/couchcord
Source0:        %{url}/archive/v%{version}/%{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.xz

BuildRequires:  cargo-rpm-macros >= 24
BuildRequires:  systemd-rpm-macros
Requires:       systemd
%{?systemd_requires}
# The daemon only *controls* Discord over local RPC and renders through the
# gamescope external-overlay atom — both are runtime peers, not hard deps.
Recommends:     discord
Recommends:     gamescope

%description
couchcord is a controller-driven Discord voice control + activity overlay for
a gamescope Big Picture / game-mode session: browse and join voice channels,
leave, and see who's talking, from the couch, while a game runs, without
Discord ever being a focus-stealing window. Ships the couchcordd user daemon
(doctor + run subcommands). Configuration lives in
~/.config/couchcord/config.toml (see the packaged config.toml.example).

%prep
# -a1 unpacks the vendor tarball (vendor/ at its root) into the source dir.
%autosetup -p1 -a1
%cargo_prep -v vendor

%build
%cargo_build
%{cargo_license_summary}
%{cargo_license} > LICENSE.dependencies

%install
# Not %%cargo_install: the workspace root is a virtual manifest (no root
# [package]), and %%cargo_install hardcodes `cargo install --path .`, which
# cargo rejects on virtual manifests. %%cargo_build (cargo-rpm-macros >= 24)
# builds with the injected `rpm` profile, so the binary lands in target/rpm/.
install -Dpm0755 target/rpm/couchcordd %{buildroot}%{_bindir}/couchcordd
install -Dpm0644 dist/couchcordd.service %{buildroot}%{_userunitdir}/couchcordd.service

%if %{with check}
%check
%cargo_test
%endif

%post
%systemd_user_post couchcordd.service
if [ $1 -eq 1 ]; then
    cat <<'EOF'
couchcord: two manual steps remain —
  1. evdev access:   sudo usermod -aG input $USER   (then log out / back in)
  2. enable service: systemctl --user enable --now couchcordd
Config: copy %{_docdir}/couchcord/config.toml.example to
~/.config/couchcord/config.toml. See the packaged SETUP.md.
EOF
fi

%preun
%systemd_user_preun couchcordd.service

%postun
%systemd_user_postun_with_restart couchcordd.service

%files
%license LICENSE LICENSE.dependencies
%doc README.md config.toml.example docs/SETUP.md
%{_bindir}/couchcordd
%{_userunitdir}/couchcordd.service

%changelog
* Thu Jul 02 2026 Mason Rhodes <mrhodesdev@gmail.com> - 0.1.0-1
- Initial packaged release: couchcordd daemon, user unit, config example.
  Discord live-validation is still pending; the config schema may change.

# Maintainer: Mason Rhodes <mrhodesdev@gmail.com>
# Build the binary first (no toolchain on SteamOS):
#   distrobox enter builder -- bash -lc 'cd ~/Projects/couchcord && ~/.cargo/bin/cargo build --release'
# Then package:  distrobox enter packager -- makepkg -fd
# Install: pacman --root ~/.local/share/deck-pkgs --dbpath ~/.local/share/deck-pkgs/var/lib/pacman -U couchcord-*.pkg.tar.zst
pkgname=couchcord
pkgver=0.1.0
pkgrel=1
pkgdesc="Controller-driven Discord voice control + activity overlay for Steam game mode"
arch=(x86_64)
url="https://github.com/MasonRhodesDev/couchcord"
license=(MIT)
depends=(deck-tenant)
optdepends=(bash python flatpak) # host-provided on SteamOS

package() {
    cd "$startdir"
    [ -x target/release/couchcordd ] || { echo "build the binary first (see PKGBUILD header)"; return 1; }
    install -Dm755 target/release/couchcordd "$pkgdir/usr/bin/couchcordd"
    install -Dm755 assets/game-mode-discord  "$pkgdir/usr/bin/game-mode-discord"
    install -Dm644 assets/systemd/couchcordd.service \
        "$pkgdir/usr/lib/systemd/user/couchcordd.service"
    install -Dm644 assets/systemd/couchcord-autostart-guard.path \
        "$pkgdir/usr/lib/systemd/user/couchcord-autostart-guard.path"
    install -Dm644 assets/systemd/couchcord-autostart-guard.service \
        "$pkgdir/usr/lib/systemd/user/couchcord-autostart-guard.service"
    install -Dm644 config.toml.example "$pkgdir/usr/share/couchcord/config.toml.example"
    install -Dm755 assets/rewire-steam-shortcuts.py "$pkgdir/usr/share/couchcord/rewire-steam-shortcuts.py"
    install -Dm644 README.md "$pkgdir/usr/share/doc/couchcord/README.md"
}

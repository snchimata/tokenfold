#!/bin/sh
set -eux

ARCH="$(uname -m)"
VERSION="${VERSION:-$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)}"

export ARCH VERSION
export UPINFO="gh-releases-zsync|${GITHUB_REPOSITORY%/*}|${GITHUB_REPOSITORY#*/}|latest|*$ARCH.AppImage.zsync"
export OUTPATH="$PWD/dist"
export OUTNAME=tokenfold-"$VERSION"-anylinux-"$ARCH".AppImage
export ICON="DUMMY"
export DESKTOP="DUMMY"
export MAIN_BIN="/usr/bin/tokenfold"

pacman -Syu --noconfirm --needed rustup git
rustup toolchain install 1.98 --profile minimal

cargo build --release --locked -p tokenfold-cli
install -Dm755 target/release/tokenfold /usr/bin/tokenfold

quick-sharun $MAIN_BIN
quick-sharun --make-appimage
quick-sharun --simple-test ./dist/*.AppImage

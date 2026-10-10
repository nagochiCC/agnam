#!/bin/bash
# Run in the pinned Ubuntu container with an already verified checkout snapshot.
set -euo pipefail
recipe=$(cd -- "$(dirname -- "$0")" && pwd)
[[ $PWD == /build ]]
[[ $(. /etc/os-release; echo "$ID:$VERSION_ID") == ubuntu:24.04 ]]
[[ $(uname -m) == x86_64 ]]
[[ ! -e AppDir && ! -e dist && ! -e toolchain ]]
[[ $PACKAGING_COMMIT =~ ^[0-9a-f]{40}$ ]]
mkdir logs validation
export SOURCE_DATE_EPOCH
SOURCE_DATE_EPOCH=$(python3 -c 'import json; print(json.load(open("source-authority.json"))["source_date_epoch"])')
export BUILD_JOBS=${BUILD_JOBS:-2}
python3 "$recipe/development.py" fetch /build/input
python3 "$recipe/development.py" rust /build/input > logs/rust-install.log 2>&1
bash "$recipe/native.sh" /build/input /build/native libraries > logs/native.log 2>&1
bash "$recipe/native.sh" /build/input /build/runtime runtime > logs/runtime.log 2>&1
export PATH="/build/toolchain/bin:$PATH"
export CARGO_HOME=/build/cargo-home CARGO_TARGET_DIR=/build/target
export PKG_CONFIG_PATH=/build/native/prefix/lib/pkgconfig
export LD_LIBRARY_PATH=/build/native/prefix/lib
export RUSTFLAGS='--remap-path-prefix=/build=/usr/src/agnam-appimage'
app=/build/agnam-source/agnam-$PACKAGING_COMMIT
[[ -d $app ]]
(cd "$app"; cargo build --release --locked --offline -j "$BUILD_JOBS") > logs/agnam-build.log 2>&1
(cd "$app"; cargo test --release --locked --offline -j "$BUILD_JOBS" archive:: -- --test-threads=2) > logs/archive-tests.log 2>&1
(cd "$app"; cargo test --release --locked --offline -j "$BUILD_JOBS" thumbnail -- --test-threads=2) > logs/thumbnail-tests.log 2>&1
(cd "$app"; cargo check --locked --offline -j "$BUILD_JOBS") > logs/cargo-check.log 2>&1
probe_config=$(pkg-config --cflags --libs gtk4 libadwaita-1 libturbojpeg libwebp fontconfig)
read -r -a probe_flags <<< "$probe_config"
cc "$recipe/probe.c" -o nogui-probe "${probe_flags[@]}"
./nogui-probe --make-fixtures /build/fixtures
python3 "$recipe/development.py" appdir /build > logs/appdir.log 2>&1
python3 "$recipe/validate.py" --development payload /build/AppDir /build/validation/appdir-elf.json
python3 "$recipe/development.py" sources /build > logs/corresponding-source.log 2>&1
env -u SOURCE_DATE_EPOCH mksquashfs AppDir payload.squashfs -noappend -comp zstd -all-root \
    -mkfs-time "$SOURCE_DATE_EPOCH" -all-time "$SOURCE_DATE_EPOCH" -processors "$BUILD_JOBS" > logs/squashfs.log
cat runtime/runtime-x86_64 payload.squashfs > candidate.AppImage
chmod +x candidate.AppImage
python3 "$recipe/validate.py" --development image /build/candidate.AppImage /build > logs/image-validation.log 2>&1
tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner \
    -cJf candidate-source.tar.xz corresponding-source
python3 "$recipe/validate.py" --development source /build/candidate-source.tar.xz /build > logs/source-rebuild.log 2>&1
mkdir dist
cp candidate.AppImage dist/Agnam-development-x86_64.AppImage
cp candidate-source.tar.xz dist/Agnam-development-AppImage-corresponding-source.tar.xz
(cd dist
    sha256sum Agnam-development-x86_64.AppImage > Agnam-development-x86_64.AppImage.sha256
    sha256sum Agnam-development-AppImage-corresponding-source.tar.xz > Agnam-development-AppImage-corresponding-source.tar.xz.sha256
    sha256sum -c Agnam-development-x86_64.AppImage.sha256
    sha256sum -c Agnam-development-AppImage-corresponding-source.tar.xz.sha256)
cp -r validation dist/validation
cp binary-provenance.json file-manifest.json ubuntu-packages.json source-authority.json development-build.json dist/

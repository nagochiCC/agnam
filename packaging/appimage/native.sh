#!/bin/bash
# Usage: native.sh INPUT_DIRECTORY BUILD_DIRECTORY [runtime|libraries|all]
# No network access. Each invocation requires a new build directory.
set -euo pipefail
recipe=$(cd -- "$(dirname -- "$0")" && pwd)
inputs=$(realpath "$1")
work=$(realpath -m "$2")
mode=${3:-all}
[[ $mode == runtime || $mode == libraries || $mode == all ]]
[[ ! -e $work ]] || { echo "Build directory already exists: $work" >&2; exit 1; }
python3 "$recipe/package.py" verify-inputs "$inputs" "$mode"
mkdir -p "$work/src" "$work/logs"
jobs=${BUILD_JOBS:-2}
export SOURCE_DATE_EPOCH=1791094136
export CFLAGS="-O2 -fPIC -ffile-prefix-map=$work=/usr/src/agnam-appimage"
export CXXFLAGS="$CFLAGS"
extract() {
    mkdir -p "$work/src/$1"
    tar -xf "$inputs/$2" -C "$work/src/$1" --strip-components=1
}
if [[ $mode != runtime ]]; then
    prefix="$work/prefix"
    extract turbo libjpeg-turbo-3.2.0.tar.gz
    cmake -S "$work/src/turbo" -B "$work/turbo" -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX="$prefix" -DCMAKE_INSTALL_LIBDIR=lib \
        -DENABLE_STATIC=OFF -DWITH_SIMD=ON -DREQUIRE_SIMD=ON
    cmake --build "$work/turbo" -j "$jobs"
    cmake --install "$work/turbo"
    grep -qx 'WITH_SIMD:BOOL=ON' "$work/turbo/CMakeCache.txt"
    extract cairo cairo-1.18.6.tar.xz
    meson setup "$work/cairo" "$work/src/cairo" --prefix="$prefix" --libdir=lib \
        -Dbuildtype=release -Dlzo=disabled -Dtests=disabled
    meson compile -C "$work/cairo" -j "$jobs"
    meson install -C "$work/cairo"
    # dpkg-source applies the complete Ubuntu quilt series, not just the orig tar.
    (cd "$work/src"; dpkg-source --no-check -x \
        "$inputs/tiff_4.5.1+git230720-4ubuntu2.5.dsc" tiff)
    grep -q 'option(jbig ' "$work/src/tiff/cmake/JBIGCodec.cmake"
    # Preserve Ubuntu's hardening policy as well as its source patches.
    export DEB_BUILD_MAINT_OPTIONS=hardening=+all
    cmake -S "$work/src/tiff" -B "$work/tiff" -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_C_FLAGS="$(dpkg-buildflags --get CFLAGS) $(dpkg-buildflags --get CPPFLAGS) $CFLAGS -Wall -pedantic -D_REENTRANT" \
        -DCMAKE_CXX_FLAGS="$(dpkg-buildflags --get CXXFLAGS) $(dpkg-buildflags --get CPPFLAGS) $CXXFLAGS" \
        -DCMAKE_SHARED_LINKER_FLAGS="$(dpkg-buildflags --get LDFLAGS) -Wl,--as-needed" \
        -DCMAKE_INSTALL_PREFIX="$prefix" -DCMAKE_INSTALL_LIBDIR=lib -Djbig=OFF \
        -Dtiff-tools=OFF -Dtiff-tests=OFF -Dtiff-contrib=OFF -Dtiff-docs=OFF
    cmake --build "$work/tiff" -j "$jobs"
    cmake --install "$work/tiff"
    grep -qx 'jbig:BOOL=OFF' "$work/tiff/CMakeCache.txt"
    (cd "$work/src"; dpkg-source --no-check -x \
        "$inputs/fontconfig_2.15.0-1.1ubuntu2.dsc" fontconfig)
    # Keep Ubuntu patches/hardening/defaults. A relative template directory uses
    # FONTCONFIG_PATH rather than scanning a newer host's conf.avail separately.
    CFLAGS="$(dpkg-buildflags --get CFLAGS) $(dpkg-buildflags --get CPPFLAGS) $CFLAGS" \
    LDFLAGS="$(dpkg-buildflags --get LDFLAGS) -Wl,-O1 -Wl,-z,defs" \
        meson setup "$work/fontconfig" "$work/src/fontconfig" \
        --prefix=/usr --libdir=lib --sysconfdir=/etc --default-library=shared \
        --wrap-mode=nodownload -Dbuildtype=release -Ddoc=disabled -Dtests=disabled \
        -Dtools=disabled -Dnls=disabled -Dcache-build=disabled \
        -Dtemplate-dir=conf.avail -Dadditional-fonts-dirs=/usr/local/share/fonts \
        -Ddefault-sub-pixel-rendering=rgb
    meson compile -C "$work/fontconfig" -j "$jobs"
    grep -qx '#define FC_TEMPLATEDIR "conf.avail"' "$work/fontconfig/config.h"
    install -m755 "$work/fontconfig/src/libfontconfig.so.1" "$prefix/lib/libfontconfig.so.1"
    readelf -d "$prefix/lib/libtiff.so.6" > "$work/logs/tiff-elf.txt"
    if grep -qi jbig "$work/logs/tiff-elf.txt"; then echo "JBIG dependency retained" >&2; exit 1; fi
    readelf -d "$prefix/lib/libcairo.so.2" > "$work/logs/cairo-elf.txt"
    if grep -qi lzo "$work/logs/cairo-elf.txt"; then echo "LZO dependency retained" >&2; exit 1; fi
    # NASM source paths are independent of GCC prefix-map; omit debug-only data.
    while IFS= read -r -d "" library; do
        strip --strip-debug "$library"
    done < <(find "$prefix/lib" -type f -name "*.so*" -print0)
fi
if [[ $mode != libraries ]]; then
    prefix="$work/runtime-prefix"
    extract musl musl-1.2.5.tar.gz
    (cd "$work/src/musl"; ./configure --prefix="$prefix" --disable-shared CC=gcc
        make -j "$jobs"; make install)
    export CC="$prefix/bin/musl-gcc"
    export PKG_CONFIG_LIBDIR="$prefix/lib/pkgconfig"
    export CFLAGS="-Os -fPIC -static -ffunction-sections -fdata-sections -ffile-prefix-map=$work=/usr/src/agnam-appimage"
    extract zlib zlib-1.3.2.tar.xz
    (cd "$work/src/zlib"; ./configure --static --prefix="$prefix"; make -j "$jobs"; make install)
    extract zstd zstd-1.5.6.tar.gz
    make -C "$work/src/zstd/lib" -j "$jobs" libzstd.a CC="$CC" CFLAGS="$CFLAGS"
    install -m644 "$work/src/zstd/lib/libzstd.a" "$prefix/lib/"
    install -m644 "$work/src/zstd/lib/zstd.h" "$prefix/include/"
    extract fuse fuse-3.15.0.tar.xz
    extract runtime runtime-source.tar.gz
    patch -d "$work/src/fuse" -p1 < "$work/src/runtime/patches/libfuse/mount.c.diff"
    meson setup "$work/fuse" "$work/src/fuse" --prefix="$prefix" --libdir=lib \
        --default-library=static -Dexamples=false -Dtests=false -Dutils=false \
        -Dudevrulesdir='' -Dinitscriptdir=''
    meson compile -C "$work/fuse" -j "$jobs"
    meson install -C "$work/fuse"
    extract squashfuse squashfuse-0.5.2.tar.gz
    (cd "$work/src/squashfuse"; ./autogen.sh
        CPPFLAGS="-I$prefix/include" LDFLAGS="-L$prefix/lib -static" \
            ./configure --prefix="$prefix" --disable-shared --enable-static \
            --without-lzo --without-lzma --without-lz4
        make -j "$jobs"; make install)
    install -d "$prefix/include/squashfuse"
    install -m644 "$work/src/squashfuse/"*.h "$prefix/include/squashfuse/"
    extract mimalloc mimalloc-2.1.7.tar.gz
    cmake -S "$work/src/mimalloc" -B "$work/mimalloc" \
        -DCMAKE_C_COMPILER="$CC" -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX="$prefix" -DMI_BUILD_SHARED=OFF \
        -DMI_BUILD_OBJECT=OFF -DMI_BUILD_TESTS=OFF
    cmake --build "$work/mimalloc" -j "$jobs"
    cmake --install "$work/mimalloc"
    # Explicit musl rcrt1 supplies static PIE startup; no host glibc CRT is used.
    # Preserve the upstream flags/linker script; version is the full source commit.
    cd "$work/src/runtime/src/runtime"
    clang -std=gnu99 -Os -fPIE -nostdinc -isystem "$prefix/include" \
        -I"$prefix/include/fuse3" -I"$prefix/include/squashfuse" \
        -D_FILE_OFFSET_BITS=64 -DGIT_COMMIT='"8f39b89e2ac31e1640b3d3f7e9a5108e6ce805fa"' \
        -ffile-prefix-map="$work"=/usr/src/agnam-appimage \
        -ffunction-sections -fdata-sections -Wall -Werror \
        -nostdlib -static-pie -Wl,--gc-sections -Wl,-T,data_sections.ld \
        -Wl,-Map,"$work/logs/runtime-link.map" "$prefix/lib/rcrt1.o" runtime.c \
        -L"$prefix/lib" -lsquashfuse -lsquashfuse_ll -lzstd -lz -lfuse3 \
        "$prefix/lib/mimalloc-2.1/libmimalloc.a" -lc "$(gcc -print-libgcc-file-name)" -o "$work/runtime-x86_64"
    readelf -l "$work/runtime-x86_64" > "$work/logs/runtime-program-headers.txt"
    if grep -q INTERP "$work/logs/runtime-program-headers.txt"; then echo "Dynamic runtime interpreter" >&2; exit 1; fi
    readelf -d "$work/runtime-x86_64" > "$work/logs/runtime-dynamic.txt"
    if grep -q NEEDED "$work/logs/runtime-dynamic.txt"; then echo "Dynamic runtime dependency" >&2; exit 1; fi
    strip --strip-debug --strip-unneeded "$work/runtime-x86_64"
    printf 'AI\002' | dd of="$work/runtime-x86_64" bs=1 count=3 seek=8 conv=notrunc status=none
    "$work/runtime-x86_64" --appimage-version
fi

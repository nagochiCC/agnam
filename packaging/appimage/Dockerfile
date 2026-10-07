FROM ubuntu@sha256:f610ab94648195aa356059f5b41d6085c9d4d903c072430cdd1af7bdb646106b AS build
# Bootstrap TLS from a hash-verified Ubuntu CA bundle before APT is available.
ADD --checksum=sha256:6bac2a01979e210d9eac1d4d56747ec709ea60654744d66705dc3c36e7629e50 https://snapshot.ubuntu.com/ubuntu/20261004T000000Z/pool/main/c/ca-certificates/ca-certificates_20260601~24.04.1_all.deb /tmp/ca-certificates.deb
RUN dpkg-deb -x /tmp/ca-certificates.deb /tmp/ca-bootstrap \
    && mkdir -p /etc/ssl/certs \
    && cat /tmp/ca-bootstrap/usr/share/ca-certificates/mozilla/*.crt > /etc/ssl/certs/ca-certificates.crt
# Ubuntu 24.04 amd64; signed, immutable package snapshot, including deb-src.
RUN printf '%s\n' \
    'Types: deb deb-src' \
    'URIs: https://snapshot.ubuntu.com/ubuntu/20261004T000000Z/' \
    'Suites: noble noble-updates noble-security' \
    'Components: main universe restricted multiverse' \
    'Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg' \
    > /etc/apt/sources.list.d/ubuntu.sources \
    && apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get -y upgrade \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
       ca-certificates curl python3 build-essential pkg-config cmake meson ninja-build \
       clang nasm gperf autoconf automake libtool patch dpkg-dev binutils patchelf file \
       squashfs-tools desktop-file-utils libgtk-4-dev libadwaita-1-dev \
       libturbojpeg0-dev liblzma-dev libbz2-dev libzstd-dev libwebp-dev \
       libdeflate-dev liblerc-dev librsvg2-common webp-pixbuf-loader \
       libglib2.0-bin libgdk-pixbuf2.0-bin dconf-gsettings-backend \
       gsettings-desktop-schemas adwaita-icon-theme shared-mime-info \
       libgl1 libegl1 libgl1-mesa-dri
ENV LANG=C.UTF-8 LC_ALL=C.UTF-8 TZ=UTC SOURCE_DATE_EPOCH=1791094136
WORKDIR /build

# Validation only: package tool versions are recorded; they do not enter the payload.
FROM debian@sha256:7792b1f7702a86946cd518db72b6a407302c3e9bc1635634368b878189e8221c AS debian-validation
RUN apt-get update && apt-get install -y --no-install-recommends \
    python3 binutils desktop-file-utils libgl1 libegl1 libgl1-mesa-dri \
    libx11-6 libxcb1 libwayland-client0 ca-certificates fonts-dejavu-core
WORKDIR /build

# Agnam 0.9.0 AppImageの正式候補ビルド

この構成はUbuntu 24.04 / x86_64で正式候補と対応ソースを生成する。
GitHub Releaseの作成・更新は行わない。公開前にActionsでの成功結果、
通知・対応ソース、今回生成した候補の実機GUIを確認する。Issue #1は最終確認までopenとする。

## ビルドの入力と出所

- Agnam: 既存 `v0.9.0`、commit `c5bddcac92995816f0f0cb212db8b11dadf52c2d`。
- タグarchive: `https://github.com/nagochiCC/agnam/archive/refs/tags/v0.9.0.tar.gz`。
- SHA-256: `aac6269c7621efe565d1a639f790f60acf3e40bb115f60802a4befa47f6301dc`。
- `package.py authority` はGitHub APIでannotated tagをcommitまで解決し、
  固定commitとarchiveのSHA-256を両方確認する。不一致・取得失敗は停止する。
- このdirectoryの処理はv0.9.0タグ外の配布実装である。
  Actionsの `github.sha` と配布処理のファイルhashを対応ソースへ記録する。
  Agnam本体は検証したタグarchiveへ、下記のGTK 4.14互換patchだけを適用して
  `cargo build --release --locked --offline` でビルドする。タグと元archiveは変更しない。
  Cargoの取得は先に `cargo vendor --locked` で行い、Cargo.lockのpackage checksumと
  vendorの全ファイルhash・全packageの収録を照合する。
- Rust 1.97.1のrustc / Cargo / rust-std / rust-src / rust-docsは公式配布tarとSHA-256で固定。
  通知には `COPYRIGHT-library.html` と参照先licensesを使う。対応ソースにはrust-srcを含む。
- Ubuntu 24.04 amd64公式OCI manifestをdigestで固定する。具体的な値は
  `Dockerfile` / `inputs.json` にある。Ubuntuパッケージは
  `20261004T000000Z` の署名付きスナップショットから導入する。
  初期TLS証明書もUbuntuの `.deb` をSHA-256で固定して取得する。
- Ubuntu由来の収録ファイルはdpkg所有者・binary version・source package/versionを記録する。
  同じ署名付きスナップショットから正確なversionの `.dsc` / orig / Ubuntu差分を取得し、
  `dpkg-source` で展開・patch適用を確認する。対応する版が取れなければ停止する。

固定した条件から再生成できることが目的であり、独立した2回の全ビルドでの
ビット単位の一致を保証した構成ではない。日時・所有者・SquashFSの時刻・tar順序を固定し、
コンパイル時のローカルビルドパスを置換する。Ubuntu OpenSSLのコンパイラflagとlibrsvgのRust診断に埋め込まれた
配布元のパスは原本hashとともに記録して保持する。
検証は記録したこの原文だけを許可し、新しいローカルパスの混入は拒否する。ホストkernel、Docker/Podman、Actions runner自体は
固定していない。これらは記録・非GUI検証で影響を確認する。

## GitHub Actions

`.github/workflows/appimage.yml` の **AppImage v0.9.0 candidate** を
この配布実装が存在するrefから `workflow_dispatch` で起動する。
入力欄からAgnamのrefや任意のscriptを指定する仕組みは設けていない。
既存タグへのpushは不要。外部Actionは完全なcommit SHA、権限は `contents: read`。

Ubuntuでのビルド、AppImage検証、対応ソースだけからの再ビルド、ネットワークを切った新しいコンテナでのruntime再ビルドに加え、
Debian 13のdigest固定コンテナでも検証してから **Agnam-0.9.0-AppImage-candidate** を保存する。
Debian検証用のツールとホスト側ライブラリは署名付きDebian repositoryから導入し、
version一覧を保存する。検証環境のAPT時点は固定していないが、生成物には入らない。
失敗時は正式候補をuploadせず、ログと検証結果だけを別の診断artifactに保存する。

## ローカルのコンテナビルド

x86_64 Linux上のDocker（BuildKit対応）が必要。Podmanでも同じDockerfileと
mountを使えるが、rootlessの所有者mappingや `ADD --checksum` 対応は利用versionで確認する。
最低でも十数GiBの空き領域を用意する。ソースとビルド出力をGitの管理対象に置かない。

repository rootから、毎回新しいdirectoryで実行する:

```sh
docker build --target build -t agnam-appimage-build packaging/appimage
mkdir -p target/appimage-local
docker run --rm \
  --mount "type=bind,src=$PWD/packaging/appimage,dst=/recipe,readonly" \
  --mount "type=bind,src=$PWD/target/appimage-local,dst=/build" \
  --env BUILD_JOBS=2 \
  agnam-appimage-build bash /recipe/build.sh
```

`/build`に以前の出力がある場合は停止する。途中で失敗したdirectoryは診断用に保持し、
修正後の正式確認は新しいdirectoryで行う。GUI、ホストのXDG保存先、秘密情報をmountしない。
ビルド終了後の `dist/`:

```text
Agnam-0.9.0-x86_64.AppImage
Agnam-0.9.0-x86_64.AppImage.sha256
Agnam-0.9.0-AppImage-corresponding-source.tar.xz
Agnam-0.9.0-AppImage-corresponding-source.tar.xz.sha256
binary-provenance.json
file-manifest.json
ubuntu-packages.json
source-authority.json
validation/
```

Actions artifactのZIPでは実行権限が保持されないことがあるため、取得後は
checksumを確認して `chmod +x Agnam-0.9.0-x86_64.AppImage` を実行する。
AppImage内部のAgnam / AppRunの実行権限はSquashFS内で保持する。

## ネイティブ依存とAppDir

`native.sh` はhash検証済みのローカル入力だけを使う。

- TurboJPEG: libjpeg-turbo 3.2.0、`WITH_SIMD=ON` / `REQUIRE_SIMD=ON`、NASMを使用。
  ビルドcacheのSIMD設定、収録バイナリhash、TurboJPEG 3の実APIを確認する。
- Cairo: 1.18.6、Meson `-Dlzo=disabled`。LGPL-2.1を選択し、元のCOPYING一式を保持。
- TIFF: Ubuntu `4.5.1+git230720-4ubuntu2.5` の全quilt patchとUbuntuのhardening設定を維持し、CMake `-Djbig=OFF`。
  他のcodecは利用可能なUbuntu開発依存で維持する。stock libtiffの後からの削除ではない。
- Fontconfig: 同じsnapshotのUbuntu `2.15.0-1.1ubuntu2` の `.dsc` / orig / Ubuntu差分を
  SHA-256で固定する。全quilt patch、hardening、RGB既定を維持し、Mesonの公式option
  `-Dtemplate-dir=conf.avail` で再構築する。版とライセンス方針は変更しない。
  stock libraryは初期化時にhostの `/usr/share/fontconfig/conf.avail` を別途解析するため、
  `FONTCONFIG_PATH` の指定だけでは新しいhost設定の警告を解消できない。
- runtime: type2-runtime commit `8f39b89e2ac31e1640b3d3f7e9a5108e6ce805fa`。
  UbuntuのGCCでmusl 1.2.5と全静的依存をビルドし、上流同様Clangでruntimeをビルドする。
  libfuse 3.15.0に上流AppImageの `mount.c.diff` を適用し、squashfuse 0.5.2、
  Zstd 1.5.6、zlib 1.3.2、mimalloc 2.1.7をリンクする。
  上流のlinker script / static PIE構成を維持し、muslの `rcrt1.o` を明示する。
  GCCのsupport libraryの出所も記録し、対応するUbuntu GCC sourceを含める。
  Alpineの事前ビルド済みライブラリやダウンロード済みruntimeを流用しない。
  ELFのINTERP / DT_NEEDEDがないこと、実行できること、link mapを確認する。
- Type 2の生成は、上記runtimeとUbuntu `mksquashfs` の出力を結合する。
  制作専用のlinuxdeploy / GTK plugin / appimagetoolは必要ない。

`ldd`で必要な共有ライブラリをたどり、GTK4、libadwaita、GLib/GIO、Pango、
Graphene、Cairo、GdkPixbuf、SVG/WebP loader、librsvg、libwebp、TurboJPEG、
liblzma、bzip2、libstdc++、Fontconfig/FreeType/HarfBuzz/FriBidi等を収録する。
別途dconf module、GSettings schema、Adwaita icon theme、MIME元XMLを収録し、cacheを再生成する。
全ELFの実際の収録一覧は `validation/elf.json`、出所は `binary-provenance.json`。

glibc、dynamic loader、libgcc_s、X11/XCB/Wayland client、GPUのGL/EGL/DRM/GBM/Vulkan等は
ホストに残す。ホストにはこれらとglibc 2.39以上が必要。
`AppRun` は収録library・data・schema・loader・GIO・Fontconfigの参照先と、存在するホストのGTK4 module探索先を設定する。
themeや表示backendを強制せず、XDG_CONFIG_HOME / XDG_CACHE_HOME / XDG_DATA_HOMEを変更しない。
Ubuntu GTKのmultiarch探索先に加え、存在する場合だけ `/usr/lib/gtk-4.0` を
既存 `GTK_PATH` の後ろに追加する。`GTK_IM_MODULE` は保持し、IME自体やホストmoduleは収録しない。
ホストmoduleの互換性はホストの版に依存する。ArchのFcitx5についてはmoduleの読込みと
`fcitx` context選択を確認し、その修正候補の日本語変換・確定・削除・フォーカス移動もArch実機で確認済み。
GTK / GLib / libadwaitaの翻訳カタログは収録しないため、標準dialogの一部が英語になる可能性がある。

Fcitx修正候補ではfile chooserの外部媒体更新・読取りも実機確認済みで、mountinfo警告は再現していない。
mount monitorへの追加対策は行わない。警告の原因と比較実測は
[`実機警告の調査`](../../docs/research/issue-1-appimage-release-build.md#arch実機の起動警告)を参照する。

### Fontconfig設定とGTK 4.14互換patch

同じ固定Ubuntu環境の `/etc/fonts/fonts.conf`、有効な `conf.d/*.conf`、
`/usr/share/fontconfig/conf.avail/*.conf` をAppDirの `etc/fonts/` へ収録する。
symlinkは原本へ解決してコピーし、原本のUbuntu binary/source package、版、hashを
`binary-provenance.json` に記録する。Fontconfig本体と設定のsource versionは一致を検証する。
DejaVu由来のルールも独立したpackageとして通知・対応ソースに含める。
`AppRun` の `FONTCONFIG_PATH` / `FONTCONFIG_FILE` と相対template-dirにより、
デフォルトのsystem設定とtemplateの両方がAppDir内で解決する。
`51-local.conf` がhostの `local.conf` へfallbackしないよう、空の `etc/fonts/local.conf` を生成する。
hostのsystemルールは読まないが、実フォントの `/usr/share/fonts`、`/usr/local/share/fonts`、
XDG user fonts、`~/.fonts`、通常のuser cacheは維持する。font自体は収録しない。
`50-user.conf` のユーザー独自ルールは維持するため、それにはFontconfig 2.15互換の構文が必要。
`FONTCONFIG_SYSROOT` でhost fontの場所をAppImage内へ変更する方法は使わない。

`patches/agnam-v0.9.0-gtk414-css.patch` はnavigation railの3色だけを
`var(--sidebar-*-color)` からlibadwaita 1.5の `@sidebar_*_color` へ変更する。
light/dark/backdropの既存色を参照し、テーマや表示backendを固定しない。
元archiveのtag/commit/hash検証の後、元 `src/app/mod.rs`、patch、適用後ファイルの
SHA-256を照合し、fuzz/offset・二重適用・想定外ソースを拒否する。
current branchのRust sourceは変更しない。`agnam-patches.json` に差分の出所と前後hashを残し、
実際にコンパイルした `APP_CSS` を `usr/share/doc/agnam/APP_CSS.css` へ抽出して検証する。
通知の `SOURCE_CODE.md` と対応ソースにも元archive、patch、適用処理・記録を含める。

## 通知と対応ソース

AppImage内の入口は `usr/share/doc/agnam/` の `LICENSE`、
`THIRD_PARTY_NOTICES.md`、`SOURCE_CODE.md`。原文は `third-party/` に保存する。
収録したUbuntuファイルのcopyright、共通ライセンス本文、ネイティブソースの通知、
全Cargo.lock packageの再帰的な `LICENSE` / `LICENSES` / `NOTICE` / acknowledgements、
Rust標準ライブラリの著作権・参照ライセンスを収集する。
UnRAR 7.01の原licenseと `acknow.txt`、Intel CRC32、BLAKE2spの帰属を保持する。
UnRARは独自許諾であり、RAR/WinRAR互換圧縮器の開発禁止条件を通知から辿れる。
IJGとFreeTypeの謝辞、mimalloc、GCCの例外、アイコンの個別条件も保持する。

自動収集は通知の法的完全性の判定ではない。生成物の原文と実収録一覧を公開前に確認する。Ubuntu librsvg内のRust 1.75の標準ライブラリ通知も
別途保持する。そのコンパイラのUbuntu改訂版を診断文字列から推測してはいない。

対応ソースarchiveは、Agnamタグarchive、Cargo vendor、Rust標準ライブラリsource、
自前library/runtimeの元source、libfuse patch、Ubuntuの対応source package一式、
実ビルド設定、配布処理、version・出所一覧、全fileのSHA-256を含む。
Ubuntu差分は `.debian.tar.*` 内のbuild規則 / patchesをそのまま保持する。

次の手順で対応ソースだけからTIFF/Cairo/TurboJPEG/Fontconfig/runtimeを再ビルドできる。
コンパイラ等の前提環境は同じ固定Ubuntuコンテナを使い、実行時のネットワークは切る。
archiveはビルドcontainer内に展開する:

```sh
mkdir -p target/appimage-source-check
# 取得したarchiveをこのdirectoryへコピーしておく。
docker run --rm --network=none \
  --mount "type=bind,src=$PWD/target/appimage-source-check,dst=/build" \
  agnam-appimage-build bash -eu -c '
    tar -xJf /build/Agnam-0.9.0-AppImage-corresponding-source.tar.xz -C /build
    cd /build/corresponding-source
    sha256sum -c SHA256SUMS
    bash packaging/appimage/native.sh /build/corresponding-source/inputs /build/rebuilt all
  '
```

元のAgnamソースへ同じpatchをオフライン適用する場合は、同じ環境で
`SHA256SUMS` を確認した後、対応ソースdirectoryから実行する:

```sh
python3 - <<'PY'
from pathlib import Path
import json, sys
sys.path.insert(0, 'packaging/appimage')
import package
original = Path('inputs/v0.9.0.tar.gz')
package.require(package.digest(original) == package.LOCK['sources']['agnam']['sha256'], 'Tag archive mismatch')
app = package.extract(original, Path('/build/agnam-rebuild'))
package.apply_agnam_patch(app)
package.verify_vendor(app, Path('vendor'))
(app / '.cargo').mkdir(exist_ok=True)
(app / '.cargo/config.toml').write_text(
    '[source.crates-io]\nreplace-with = "vendored-sources"\n'
    '[source.vendored-sources]\ndirectory = ' + json.dumps(str(Path('vendor').resolve())) + '\n')
print(app)
PY
```

表示されたdirectoryにはarchive内の `vendor/` を参照する設定も作成される。固定Rust toolchain、上記再構築prefixと
`build.sh` の環境・`cargo build --release --locked --offline` を使う。
元archiveとpatchを分離しているため、未変更のタグsourceも取得できる。

libfuse差し替え時はarchiveの元tarを改変版へ置き換え、
archive内 `inputs.json` のhashを更新して `native.sh ... runtime` を実行する。
元のAppImageから `--appimage-offset` でSquashFSの開始位置を得て、
その位置以降のpayloadを新しいruntimeへ結合すれば差し替え版を作れる。
`validate.py source` はfreshなdirectoryへ展開してlibraryとruntimeを再ビルドし、
再リンクしたruntimeと元payloadを結合して `--appimage-extract-and-run --help` を実行する。
Agnam側のobjectを用意しなくてもruntimeのlibfuseを再リンクできることをこの検証で確かめる。

## 検証と公開前の確認

```sh
bash -n packaging/appimage/build.sh packaging/appimage/native.sh packaging/appimage/AppRun
python3 -m unittest discover -s packaging/appimage -p 'test_*.py'
```

ビルドではタグsourceの既存archive / thumbnail testと `cargo check` も実行する。
`validate.py` はType 2 magic、実行権限、展開内容、AppRun / icon / desktop entry、
全ELFのDT_NEEDED / RPATH / RUNPATH、未解決library、GLIBC / GLIBCXX要求、
外部symlink、絶対ビルドパス、JBIG/LZO、不要なsystem library混入を確認する。
`probe.c` はGTKの表示初期化をせず、schema、GTK/libadwaita resource、dconf、
TurboJPEG API、JPEG/PNG/WebP/SVGのGDK・Pixbuf decodeを確認する。
同梱Fontconfigの `FcInitLoadConfigAndFonts`、AppDirの設定/template探索、
日本語・emoji・等幅patternのmatch、通常のhost fontとXDG user fontの探索も確認する。
Debian検証環境のDejaVuは検査用であり、成果物へは入れない。match取得だけではglyphの有無は保証しない。
同梱GTKの `GtkCssProvider` で実際のpatched CSSとlibadwaitaのlight/dark既定色を解析し、
Fontconfig / Theme parser警告があれば検証を失敗させる。
`LD_DEBUG=libs` で収録libraryとSVG/WebP loaderが実際に読み込まれることも確認する。
GUI確認の成功をこれらの検査から推測しない。

公開前に、Actions上の成功runと4成果物のchecksum・通知・対応ソースを確認する。
今回の候補はFontconfigとCSS互換patchが変わるため、前候補の実機GUI確認をそのまま適用しない。
起動時のFontconfig / Theme parser警告が消え、日本語IME・日本語/emoji・light/dark・
navigation railのactive/non-active表示・file chooserが正常なことを再確認する。
取得した候補について最低限、起動、画像3形式とSVG icon、file chooser、
ZIP/RAR/7z等の読取、本棚・thumbnail、設定/履歴の再起動後復元を確認する。
普段使うWayland/X11環境で確認し、利用する別distributionでも必要に応じて確認する。
その後、AppImageと対応ソースを同じ公開場所から提供する別作業へ進む。

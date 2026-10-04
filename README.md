# Agnam

AgnamはRust + GTK4 / libadwaitaで開発している漫画・画像Viewerです。本棚からの閲覧と、画像・archiveを直接開く閲覧の両方に対応しています。

## 主な機能

- Single / Spread表示、ページ移動、fullscreen、Viewer Slider
- 本棚rootからの階層navigationとarchive内容の閲覧
- 本棚Cover、フォルダpreview、並び替え、読書進捗
- 検索、閲覧履歴、お気に入り
- smart crop、background preload、thumbnail生成、cache
- nested archiveとRAR / 7zの段階的読み込み
- 設定と前回閲覧位置の保存・復元

## 対応形式

画像:

- PNG
- JPG / JPEG
- WebP

archive:

- ZIP / CBZ
- RAR / CBR
- 7z / CB7
- TAR / CBT
- LZH / LHA

## Build / Run

Agnam 0.9.0は初回公開版です。sourceからbuildして利用できます。

必要な環境:

- Rust toolchain（Rust 2024 editionをbuildできるもの）
- GTK 4.10以降
- libadwaita 1.5以降
- `glib-compile-resources`
- `pkg-config`から検出できるlibturbojpeg（SIMD supportを含むもの）

build:

```sh
cargo build --release --locked
```

実行:

```sh
./target/release/agnam
```

## 基本的な使い方

- 設定で本棚rootを指定すると、Libraryからフォルダ・Book・archiveを辿って閲覧できます。
- `Ctrl+O` から画像や対応archiveを直接開けます。
- 左navigation railから本棚、検索、履歴、お気に入りへ移動できます。
- ViewerではHeaderBar、Slider、keyboard shortcutからページ移動や表示設定を操作できます。

## Development documentation

開発時のcurrent source of truthは [`docs/README.md`](docs/README.md) を入口として参照してください。

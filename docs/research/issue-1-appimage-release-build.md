# Issue #1: v0.9.0 AppImage正式候補の生成と検証

## 対象と判断

正式候補と対応ソースの生成処理を `packaging/appimage/`、手動起動のworkflowを
`.github/workflows/appimage.yml` に実装した。current branchのAgnam source / Cargo.toml / Cargo.lockは変更していない。
タグ検証後に適用するGTK 4.14互換patchはnavigation railの色指定3行だけで、下記の配布互換修正として記録する。
利用者向けの再現手順と確定した配布構成は [`../../packaging/appimage/README.md`](../../packaging/appimage/README.md) を参照する。

ローカルの固定Ubuntu環境で候補を生成し、Arch / Ubuntu 24.04 / Debian 13の非GUI検証、
対応ソースからのlibrary / runtime再ビルドと再リンクを確認した。
[Actions run 37240177389](https://github.com/nagochiCC/agnam/actions/runs/37240177389) は全工程成功し、
その候補のArch実機の主要GUI操作もユーザーが確認した。
[Fcitx修正候補のrun 37320758187](https://github.com/nagochiCC/agnam/actions/runs/37320758187) も成功。
その候補の日本語IME操作、日本語/emoji表示、light/dark、rail表示、file chooserでの外部媒体更新・読取りを
ユーザーが実機確認し、IM module / mountinfo警告は再現していない。
Fontconfig / CSS互換修正後の最終run `37574815351` も全工程成功し、その候補のArch実機GUIと
通知・対応ソースの最終確認が完了した。**AppImage v0.9.0はGitHub Release用artifactとして採用可能**と判断する。
Issue #1の配布可否調査に未解決blockerはない。branch統合とGitHub Release v0.9.0での公開まで完了している。
最終候補の識別情報と確認範囲は下記に記録し、過去のローカル実証のchecksumとは区別する。

前回の `/tmp/agnam-license-audit-cmTmqi/REPORT.ja.md`、component/file/ELF/crate台帳と
Issue #1の本文・コメントを照合した。前回の試作バイナリは正式候補へ流用していない。

## 最終Actions候補の採用判断

[run 37574815351](https://github.com/nagochiCC/agnam/actions/runs/37574815351) のrun / job / artifact APIを再確認した。
branchは `chatgpt/appimage-release`、配布実装commitは `fc647be9fbbac34cbe22ae141ab316a05826a560`。
runとbuild jobは `completed / success` で、次の全工程が成功している。

- 配布script検査と固定Ubuntu環境の作成
- 正式候補AppImage・対応ソースの生成と検証、出力所有権のrunnerへの復元
- 対応ソースarchiveのみを入力とする、新しいcontainerでのnetwork無効runtime再構築
- ホストGTKなしのDebian 13非GUI検証、検証済み候補のupload

artifactは `Agnam-0.9.0-AppImage-candidate`、ID `11463345152`、ZIPサイズ610,138,702 bytes。
APIのZIP digestは `875d1c747ce60b92af0e913640cde96461d5c4db40e18c9621932ca4d4b07d6c`。
API経由のZIP取得は使用ツールの512 MiB上限でできなかった。下記の内部hashはAPIから取得した値ではなく、
ユーザーが取得済みの最終候補を読み、それぞれの `.sha256` と実ファイルを再照合した値である。
ZIPのdigestをAppImage本体または対応ソースのhashとして扱わない。

| 最終Actions成果物 | bytes | SHA-256 |
|---|---:|---|
| `Agnam-0.9.0-x86_64.AppImage` | 39,537,024 | `d2b76662d36729966a3ca72b5e0dd6337a4b8853d30c68dc46a696ab46fbaebc` |
| `Agnam-0.9.0-AppImage-corresponding-source.tar.xz` | 566,933,312 | `b4c7f7fceba8dd503f0e32d3e5db3733178a72c673ec970692227216fb149c4e` |

4成果物（上記2ファイルと各 `.sha256`）、出所台帳と検証記録が揃っている。
最終reviewでは新しいビルド・GUI起動を行わず、既存証拠と実生成物の整合を次のように確認した。

| 確認対象 | 最終確認結果 |
|---|---|
| source authority | 現在のGitHub APIでもv0.9.0は固定commit `c5bddcac92995816f0f0cb212db8b11dadf52c2d`。未変更のタグarchiveのhashを対応ソースで照合。配布実装commitは別の環境記録に保持。 |
| 固定環境・native library | Ubuntu OCI digest / snapshotが現recipeと一致。TurboJPEG 3.2.0 SIMD、TIFF JBIG無効、Cairo LZO無効、Fontconfigの設定とUbuntu差分を維持。全102 ELFのJBIG / LZO依存0件。 |
| AppDir・ホストとの境界 | Type 2展開と全2,889ファイルのhashが台帳に一致。desktop / icon、GTK resource、schema、loader、相対RPATHとホストに残すlibraryの検査が成功。翻訳catalogは追加していない。 |
| Fontconfig・CSS | Ubuntu / DebianのprobeログでAppDirの設定とXDG user font探索、画像decode、GTK 4.14で実patched CSSとlight/dark色の解析を確認。Fontconfig / Theme parser警告0件。 |
| 対応ソース | 全13,072ファイルのSHA-256が一致し、配布処理11ファイルもcurrent recipeと一致。99 Ubuntu binary packageと83 source packageの版・収録ファイルが対応。 |
| 再構築・再リンク | `validation/source.json` でTIFF / Cairo / TurboJPEG / Fontconfig再構築、CSS patch再適用、runtime再構築と再リンクしたruntimeによるextract-and-run成功を確認。別containerのofflineログも固定runtime commitのversion実行まで成功。 |
| 通知 | Agnam MIT、Ubuntu packageごとのcopyright、194 Cargo packageの通知一覧、Rust標準library、native / runtimeの原文を実生成物で確認。UnRARの独自license・acknowledgements・Intel CRC32・BLAKE2sp、`crc-catalog` / `nt-time` の `LICENSES/`、mimalloc、IJG / FreeType、GCC例外等を保持。既存の選択条件と矛盾する変更はない。 |

対応ソースには元v0.9.0 archive、GTK 4.14互換patchと適用前後hash、Cargo vendor、Rust source、
native / runtimeの固定入力、Fontconfigを含むUbuntu orig / 差分 / `.dsc`、実ビルド設定、
libfuse patch、link map、再ビルド・再リンク手順、source / binary出所台帳がある。
自動収集だけを法的完全性の保証とせず、既存reviewの方針と実際の収録物の整合を確認した。
公開時はこのAppImage・対応ソース・各checksumを同じダウンロード場所から提供する。

[ユーザーの最終Arch実機報告](https://github.com/nagochiCC/agnam/issues/1#issuecomment-6032283702)では、
checksum照合後、isolated XDG環境で日本語IME、日本語/emoji、light/dark、navigation railの
active/non-active表示、file chooserが正常。Fontconfig / Theme parser / Fcitx IM / mountinfo GIO警告は再現なし。
外部媒体の接続・取り外し時の一覧更新と媒体上fileの読取りは、前のFcitx修正候補で確認済みであり、
最終候補で再実施したとは扱わない。PNG/JPEG/WebP、ZIP/RAR/7z、本棚・thumbnail、設定・履歴の
再起動後復元も先行候補の実機確認として保持する。今回の互換修正はそれらの処理を変更していない。

最終stderrに残るRAR thumbnailの `single archive entry exceeds its byte limit` は
current仕様のsingle entry 64 MiB制限による正常拒否であり、AppImage採用のblockerではない。
安全制限、IME探索、theme/backend、XDG保存先、mount monitoringを変更しない。
この採用判断は確認済み環境と既存の互換性条件に基づく。全distribution / 将来のhost moduleの保証や、
全AppImageのビット単位再現性の保証とは区別する。

## GitHub Release公開

最終検証済み成果物は [GitHub Release v0.9.0](https://github.com/nagochiCC/agnam/releases/tag/v0.9.0) で公開した。
Releaseはdraft / prereleaseではなく、tag `v0.9.0` に対する通常Releaseである。
公開assetは次の4点で、GitHub Release APIのasset digestも最終検証値と一致した。

- `Agnam-0.9.0-x86_64.AppImage`: SHA-256 `d2b76662d36729966a3ca72b5e0dd6337a4b8853d30c68dc46a696ab46fbaebc`
- `Agnam-0.9.0-x86_64.AppImage.sha256`
- `Agnam-0.9.0-AppImage-corresponding-source.tar.xz`: SHA-256 `b4c7f7fceba8dd503f0e32d3e5db3733178a72c673ec970692227216fb149c4e`
- `Agnam-0.9.0-AppImage-corresponding-source.tar.xz.sha256`

GitHubが自動生成する `Source code (zip)` / `Source code (tar.gz)` は、上記の対応ソースarchiveとは別物として扱う。

## 固定した入力

| 入力 | 基準 |
|---|---|
| Agnam | tag `v0.9.0` / commit `c5bddcac92995816f0f0cb212db8b11dadf52c2d` |
| タグarchive SHA-256 | `aac6269c7621efe565d1a639f790f60acf3e40bb115f60802a4befa47f6301dc` |
| Ubuntu 24.04 amd64 OCI manifest | `sha256:f610ab94648195aa356059f5b41d6085c9d4d903c072430cdd1af7bdb646106b` |
| Ubuntu package snapshot | `20261004T000000Z`、Ubuntu署名付きInRelease / Sources / Packages |
| Rust | 1.97.1、公式rustc / Cargo / rust-std / rust-src / rust-docsのSHA-256を固定 |
| TurboJPEG | libjpeg-turbo 3.2.0、NASM、WITH_SIMD / REQUIRE_SIMDともON |
| Cairo | 1.18.6、Meson lzo=disabled |
| TIFF | Ubuntu 4.5.1+git230720-4ubuntu2.5、Ubuntu quilt差分とhardeningを維持、jbig=OFF |
| runtime | type2-runtime `8f39b89e2ac31e1640b3d3f7e9a5108e6ce805fa` |
| runtimeの静的依存 | libfuse 3.15.0 + AppImage mount.c.diff、squashfuse 0.5.2、musl 1.2.5、Zstd 1.5.6、zlib 1.3.2、mimalloc 2.1.7 |
| Debian 13検証OCI manifest | `sha256:7792b1f7702a86946cd518db72b6a407302c3e9bc1635634368b878189e8221c` |

全直接取得物のURL / SHA-256は `inputs.json`。Ubuntu sourceはAPTの署名付きSourcesに基づいて
正確な版を取得し、`.dsc` のSHA-256・サイズ・package/versionとorig/Ubuntu差分を照合する。
第三者source・実行toolの未検証downloadに依存しない。

既存tagのGitHub API解決結果とarchive hashを照合した。配布処理の出所はAgnamのtagとは別に記録する。
ローカル候補の `build-environment.json` は `packaging_commit=local-working-tree` とし、
archive内の処理全fileをSHA256SUMSで特定する。Actionsでは `github.sha` を記録する。

Docker/Podman・GitHubのrunner VM・共有するkernel・GPU環境はrepositoryから固定できる
コンテナ内部とは異なる境界にある。runnerの更新に追従しつつ、固定コンテナと入力hashで
コンパイラ・library条件を固定し、ELF解決・非GUI試験・実機GUIで影響を確認する。
kernelは生成する環境情報へ記録する。独立した全ビルド2回のAppImage byte一致を確認したわけではない。

## ローカル実証方法と範囲

このmachineにはDocker / Podmanがなかったため、固定OCIのlayerをhash検証して展開し、
複数UIDをmapしたuser / mount / PID namespace内でchrootした。
Ubuntu layer SHA-256は `a60ef8ec184edb5ed91f5983f00261359fb2fe20f603e8d488b9477d32fcad70`。
Dockerfileと同じ固定snapshotから実際にAPT導入し、packageの設定も完了した。
以前のsingle-UID bubblewrap環境を正式ビルドの基準へ流用したものではない。

配布処理の実ビルド中に判明した条件を修正し、別の空の作業directoryでタグからのビルドを実施した。
修正した後続工程は再実行して候補・対応ソースを作成し、最終archive内の配布処理が
その時点の `packaging/appimage/` とbyte一致することを確認した。
この実証はDocker engine / Actions artifact uploadの実行確認には相当しない。
当時必要だったActionsでの実行確認は、上記最終runで完了した。
GitHubの仕様上、workflow_dispatchはworkflowがdefault branchに存在することを要求する。
初回のbranch確認には既存作業branch `chatgpt/appimage-release` の配布関連file更新を対象とする
push triggerも用意した。default branch登録後はActionsのRun workflowから対象refを選択する。
このtrigger追加はReleaseへ書き込む権限を追加しない。
[手動起動の前提条件](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow)を参照。

## 初回ローカル候補と対応ソース

作業directoryは `target/appimage-final/`、配布候補はその `dist/` にある。
`target/` はGit管理対象外で、第三者の巨大source / download / 作業rootfsをrepositoryの管理対象へ追加していない。

| file | bytes | SHA-256 |
|---|---:|---|
| Agnam-0.9.0-x86_64.AppImage | 39,504,256 | `ce09faeba669f081d26f7c35c278fe39583c028c698bb7e9b56ea91ed81250c5` |
| Agnam-0.9.0-AppImage-corresponding-source.tar.xz | 553,420,772 | `9d195ea49a720d364c6ecc4ee1a8413f29b6f99af5ba9004121c0d355a570200` |

それぞれの `.sha256`、binary / file / source authority manifest、Ubuntu package一覧、
`validation/` を取得対象に含める。ローカルとActionsは配布処理の出所・kernel等の環境情報が異なるため、
ここに記載した対応ソースのchecksumを将来のActions生成物の期待値として固定しない。

## 初回ローカル候補のELF / 非GUI検証の証拠

候補をType 2として識別し、実行権限、`--appimage-extract`、展開内容のhashとsymlink、
`--appimage-extract-and-run --help`、desktop entry / icon / AppRun / 本体を確認した。
AppDirは2,800 file path、ELFは102。GTK等の翻訳 `.mo` は0。

全ELFでDT_NEEDED、RPATH / RUNPATH、loader解決、要求symbol versionを確認した。
最大の要求は **GLIBC 2.38 / GLIBCXX 3.4.30**。glibc・dynamic loader・GPU driver等の禁止対象は未収録。
JBIG / LZOの共有libraryとDT_NEEDEDは **0**。単にlibrary fileを削除していない。

最終 `libtiff.so.6` のDT_NEEDED:

```text
libz.so.1 libdeflate.so.0 libjpeg.so.8 libLerc.so.4 liblzma.so.5
libzstd.so.1 libwebp.so.7 libm.so.6 libc.so.6
```

TIFF / Cairo / TurboJPEGの収録元は自前prefixであり、Ubuntu stockへの取り違えを元binary hashで拒否する。
NASM由来のdebug source pathはdebug情報の除去で解消した。
Ubuntu OpenSSLのコンパイラflagとlibrsvgのRust診断には配布元の `/build/` が元から含まれる。
その2 source groupだけを原本hash・原文とともに記録し、記録に一致する既知文字列だけを保持する。
今回のローカルbuild path、非相対RPATH、AppDir外symlink、予期しないhost libraryの解決は拒否する。

Arch / Ubuntu 24.04 / Debian 13の全102 ELFの解決とAgnamのhelp実行を確認した。
DebianにはホストのGTK4 / libadwaitaを入れていない。
`probe.c` は表示初期化をせず、GTK 4.14.5 / libadwaita 1.5.0 / Cairo 1.18.6 /
TurboJPEG 3.2.0、GSettings schema、GTK / Adwaita埋込みresource、dconfを確認した。
JPEG / PNG / WebPをGDKとPixbufの双方で32×16にdecodeし、AgnamのSVGを128×128にdecodeした。
`LD_DEBUG=libs` でGTK / Adwaita / Cairo / TurboJPEG / SVG・WebP loader / dconfがAppDirから
実際に初期化されることを確認した。GPU描画、FUSE mountからの通常GUI起動、file chooser操作は確認していない。

タグsourceの既存testはarchive 128成功・2 ignored、thumbnail関連140成功・1 ignored。
`cargo build --release --locked --offline` と `cargo check --locked --offline` は成功した。
ignored testを実行済みとは扱わず、全test / GUIの検証を行ったとは扱わない。

## 初回ローカル候補の通知と対応ソースの証拠

実収録ファイルから96 Ubuntu binary package / 82 source packageを対応付けた。
194 Cargo packageの原文を保守的に収集し、LICENSES / LICENCE / acknowledgementsも保持した。
この194を全部Agnamの実行時へリンクしたという意味ではない。

Agnam MIT、GTK系・Ubuntu vendorのcopyright、共通license、IJG / FreeTypeの謝辞、
UnRAR 7.01の独自license / acknow.txt / Intel CRC32 BSD-2-Clause / BLAKE2sp、
mimalloc、Rust 1.97.1のCOPYRIGHT-library.htmlと参照licenses、
Ubuntu librsvg内のRust 1.75についての上流COPYRIGHT / MIT / Apache通知を保持した。
Rust 1.75のUbuntu compiler改訂版をpanic文字列から確定したとは扱わない。

`lzma-rust` は原crateのApache grant・作者・READMEと全文を保持する。
原文を含まないWindowsのみのwinapi import package 2件は、同じrepository / 作者の
winapi 0.3.9原licenseを補足し、manifestへ補足の根拠を記録する。
UnRARの圧縮器開発禁止条件や、Adwaitaの標準LGPL-3 / 個別CC-BY-SA-4.0条件を通知から辿れる。
自動収集が通知の法的完全性の判定になったとは扱わない。最終候補の原文と既存方針の整合確認は上記参照。

対応ソースはURL一覧だけではなく、Agnam archive / vendor / Rust library source、
自前native / runtimeの実source、AppImageのlibfuse patch、全82 Ubuntu source packageの
orig / 差分 / .dsc、build設定、配布処理、対応表、13,061 fileのSHA-256を含む。
展開・全hash・descriptor内hash・package/versionの対応・patchの存在を検証し、
そこからTIFF JBIG無効 / Cairo LZO無効 / TurboJPEG SIMD / runtimeをfreshなdirectoryへ再ビルドした。

## runtime再リンクの実証

muslと依存を固定sourceから静的にビルドし、上流同様Clangでruntimeをビルドした。
musl rcrt1を明示し、host glibcのCRTを混入させていない。GCC supportのsource / noticeも含む。
INTERP / DT_NEEDEDがなく、実行できることを確認した。静的link mapを対応ソースへ保持する。

配布する対応ソースを新しい作業directoryへ展開し、そこにある処理・入力だけで全libraryとruntimeを再構築した。
再ビルドしたruntimeへ元候補のSquashFSを結合し、実際にextract-and-runでAgnamのhelpを実行できた。
さらに元の作業directoryをmountせず、network namespaceを無効にした隔離環境でruntimeを再ビルドした。
事前ビルドしたruntime/objectの流用ではない。コンパイラ・ヘッダー等の前提ツールは固定Ubuntu環境のものを使う。

runtimeのSHA-256は元ビルド、対応ソースからの再ビルド、ネットワーク無効の再ビルドで一致した:

```text
323205e3f90a07d51ad925a43773407c5047fdab4319e2d29aae5f742835a0db
```

これはruntimeの一致を示し、AppImage全体の2回独立生成による一致を証明するものではない。
libfuseの差し替えは元source archiveと記録hashを更新して同じ静的link処理を実行できる。
Agnam本体のobjectをruntime再リンク材料として要求する構成ではない。

## 初回実装の静的検証

- Bash構文、Python構文 / 6 unittest、YAML構文・権限・Action SHA固定を確認。
- shellcheck 0.11.0、actionlint 1.7.12は警告なく成功。
- 検証用tool download SHA-256: shellcheck `8c3be12b05d5c177a04c29e3c78ce89ac86f1595681cab149b65b97c4e227198`、actionlint `8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8`。
- その後のActions・実機GUI・通知・対応ソースの最終確認は上記「最終Actions候補の採用判断」を参照。

## Zstd / mimallocの取得URLと固定hashの対応修正

[初回Actions run 37211030111](https://github.com/nagochiCC/agnam/actions/runs/37211030111) は
固定Ubuntu環境の作成に成功し、入力取得で停止した。run / job APIとIssue #1の記録を確認した。
ログ取得APIは認証なしでは403、画面もログインを要求するため、完全なログは取得できていない。
エラー本文はユーザー提供とIssueの記録を根拠とする。

Zstdの正式Release assetのURLに、API自動生成archiveのhashを組み合わせていた。
`https://api.github.com/repos/facebook/zstd/tarball/v1.5.6` を実際に取得すると、
旧固定値 `142f015292816aad5eb1e966ab09ac649f4f530fc9457b25ab78df2495fc031d` と一致した。
以前のローカル入力もこのhashで、root名は `facebook-zstd-35016bc`。
正式assetとの全通常ファイルの内容は一致するが、tarのroot名等とgzipが異なる。
既存キャッシュのhash確認だけではURLと配布形式の取り違えを発見できなかった。

[Zstandard v1.5.6 - Chrome Edition](https://github.com/facebook/zstd/releases/tag/v1.5.6) の
正式asset `zstd-1.5.6.tar.gz` は2,406,875 bytes、作成日時 `2024-03-27T00:24:25Z`。
同名の `.sha256`（84 bytes）と `.sig`（858 bytes）も確認・取得した。
タグのcommitは `794ea1b0afca0f020f4e57b6732332231fb23c70`。
[上流checksum](https://github.com/facebook/zstd/releases/download/v1.5.6/zstd-1.5.6.tar.gz.sha256) と
新規downloadの `sha256sum` が次の値で一致したため、URLとversionは維持して固定hashを修正した。

```text
8c29e06cf42aacc1eafc4077ae2ec6c6fcb96a626157e0593d5e82a34fd403c1
```

ヘッダーのversion 1.5.6と `lib/Makefile` を確認した。署名は存在確認のみで暗号学的検証は未実施。
新規取得の不一致で正式入力fileを作らず停止する単体testを追加した。

mimallocにも同じ取り違えがあった。旧固定値
`c462b927591413241faab8df8d394a8b694183c203af794e83cceb773a86efeb` は
APIの `tarball/v2.1.7` と一致し、タグarchive URLの取得hashは
`0eed39319f139afde8515010ff59baf24de9e47ea316a315398e8027d198202d`。

[上流のv2.1.7タグ](https://github.com/microsoft/mimalloc/tree/v2.1.7) をAPIで解決すると、
annotated tag `7c5c43f58b51baace21fadaeed24fdfeb2977ed0` が
commit `8c532c32c3c96e5ba1f2283e032f69ead8add00f` を指していた。
上流Release APIのv2.1.7指定は404で、Release一覧にもこのtagはなく、
固定checksum付きのRelease assetは確認できなかった。
タグ移動に影響されないよう、取得元を次のcommit指定の公式APIへ固定した。

```text
https://api.github.com/repos/microsoft/mimalloc/tarball/8c532c32c3c96e5ba1f2283e032f69ead8add00f
SHA-256: cd1f9bbfde2687b23bfe3d2e4814e293be093529f39ecc55d1a7deb643b27757
```

取得した1,181,106 bytesのarchiveはrootが `microsoft-mimalloc-8c532c3`。
全291通常ファイルの内容がv2.1.7タグarchiveと一致し、`include/mimalloc.h` の
`MI_MALLOC_VERSION 217` とreadmeのv2.1.7記載を確認した。
これは上流checksumのあるRelease assetではなくGitHubの自動生成archiveである。
[GitHubの保証範囲](https://docs.github.com/en/repositories/working-with-files/using-files/downloading-source-code-archives#stability-of-source-code-archives)
ではcommit指定でも圧縮形式の永続的なbyte一致は保証されない。
記録hashの不一致は必ず停止し、上流の変更を無条件に受け入れない。
実際に使用したarchiveを対応ソースへ保持する設計は維持する。

[途中状態のrun 37218174445](https://github.com/nagochiCC/agnam/actions/runs/37218174445) は
固定Ubuntu環境の作成に成功し、`Build and validate candidate and corresponding source` で失敗した。
公開annotationはexit code 1のみ。ログ取得APIは403のため、mimallocが実際の停止原因だったとは断定しない。
途中の固定値でmimalloc取得が停止することはローカルで再現済み。

固定Ubuntu 24.04環境の空の入力directoryから `package.py fetch` を実行し、
全21入力（199,970,593 bytes）をキャッシュなしで新規取得してhash照合に成功した。
`package.py verify-inputs` と `.dsc` 内のhash照合も成功。
Agnam / nativeのversionファイル、Rust公式配布manifestのURL・hashと収録version、runtimeのarchive rootを確認し、
他の取得元・固定値に追加の不一致は見つからなかった。
通常Release asset、GitHub自動生成archive、Ubuntu source、Rust公式配布、補足通知の区別は
URLと実際のarchive内容を照合した。補足Rust 1.75通知は上流の該当refからの新規取得をhash検証した。
新しいdirectoryで `native.sh ... all` を実行し、TurboJPEG 3.2.0 SIMD / Cairo 1.18.6 LZO無効 /
Ubuntu TIFF JBIG無効とruntimeの再ビルドが成功した。
runtimeは `--appimage-version` を実行でき、INTERP / DT_NEEDEDはなく、
SHA-256は以前の検証済みruntimeと同じ
`323205e3f90a07d51ad925a43773407c5047fdab4319e2d29aae5f742835a0db` だった。
Bash / Python構文、7単体test、shellcheck / actionlintも成功。
今回の入力修正でActionsの再確認へ進める。過去の候補・対応ソースのchecksumを
今回の処理の成果物として扱わず、正式候補はActionsで再生成・検証する。
証拠（API応答、全入力hash・version一覧、build log）はGit管理対象外の
`target/appimage-input-fix/` に保持した。

## Actionsのbind mount出力の所有者

[run 37218913472](https://github.com/nagochiCC/agnam/actions/runs/37218913472) は候補と対応ソースの
生成・検証に成功したが、続くruntime確認stepでrunner側のログ作成に失敗した。
失敗時の診断uploadも `validation/test-state` の走査で失敗した。
run / job APIで工程の結果を確認し、具体的なPermission denied / EACCESは
ユーザー提供とIssue #1の記録を照合した。

ビルドcontainerはrootでbind mountへ出力する。`validation` はrunnerが書き込めず、
検証用のXDG stateには0700 directory / 0600 fileも作られるため、readerにも所有者が必要になる。
AppImageの展開先 `extraction/squashfs-root` にも0700 directoryがある。
既存の本体ビルドはrootのまま維持し、直後の専用stepで成功・失敗にかかわらず
`dist` / `logs` / `validation` / `extraction` をrunnerのUID/GIDへ戻す。
`chown -hR` はsymlinkを辿らず、file modeと配布archive内部は変更しない。
ビルドがskipされた場合は復元stepもskipする。復元自体の失敗はworkflowを停止させる。

後続のoffline runtime確認とDebian検証は `--user "$(id -u):$(id -g)"` / `HOME=/build` で実行する。
そのため新しい `agnam-runtime-offline` の出力やDebianの検証記録もrunner所有になる。
`--network=none`、対応ソースのみの入力、全hash確認、runtime再ビルドとversion実行は維持した。
失敗時の診断uploadも維持し、chmodによる権限の拡張は行わない。

ローカルにはDockerがないため、既存の固定Ubuntu / Debian rootfsとuser namespaceで確認した。
UID/GID 12345（passwd登録なし）で元のログ作成・走査エラーを再現し、workflowから抽出した
復元処理で4つの出力領域への作成・読取り・走査が可能になった。失敗したビルドの私有診断も確認した。
保持していた対応ソースだけから、非root・network無効のruntime再ビルドとversion実行に成功した。
既存ローカル候補を使い、Debian 13でも非rootの全102 ELF解決・help・画像decode・検証記録のコピーに成功した。
これは以前のローカル対応ソースを使った権限検証であり、今回のActions成果物の検証には相当しない。
その後のActionsの全工程・upload成功と最終成果物のchecksumは上記の採用判断で確認した。
検証記録はGit管理対象外の `target/appimage-permissions-check/` に保持した。

## Arch実機の起動警告

### 対象と環境

基準は `chatgpt/appimage-release` の `0951d64280857d3225fd73233a94964fa17b3db8`。
Issue #1のGUI報告と成功runのAPIを照合した。
ユーザーが取得したartifactのchecksumファイルを再検証した:

| Actions成果物 | bytes | SHA-256 |
|---|---:|---|
| `Agnam-0.9.0-x86_64.AppImage` | 39,504,256 | `cfe69a31bc8a9e50a74b0bb69ea8497fa25b36d0c3461549b348f796f8373e48` |
| `Agnam-0.9.0-AppImage-corresponding-source.tar.xz` | 553,388,652 | `0846c401c04777fcb844c6d195365ece026fcc980705592f62f915703c405bdf` |

起動、PNG/JPEG/WebP、ZIP/RAR/7z、本棚、thumbnail、設定・履歴と再起動後の復元は
ユーザー確認済み。以下の非GUI測定をそのGUI操作の代わりとは扱わない。

| component | Archホスト | AppImage（Ubuntu binary version） |
|---|---|---|
| Fontconfig | `2:2.18.3-2` | `2.15.0-1.1ubuntu2` |
| GTK4 | `1:4.22.5-1` | `4.14.5+ds-0ubuntu0.10` |
| libadwaita | `1:1.9.4-1` | `1.5.0-1ubuntu2` |
| GLib/GIO | `2.88.3-1` | `2.80.0-6ubuntu3.9` |
| libmount | `2.42.4` | `2.39.3` |
| Fcitx5 / fcitx5-gtk | `5.1.23-1` / `5.1.7-1` | 外部IM moduleなし（simple / WaylandはGTK組込み） |

`LD_DEBUG=libs` と出所台帳で、上記同梱libraryの初期化を確認した。
この初回候補ではFontconfig設定は未同梱。ホストのGTK4 moduleは
`/usr/lib/gtk-4.0/4.0.0/immodules/libim-fcitx5.so` にある。

診断shellは `GTK_IM_MODULE=fcitx`、`QT_IM_MODULE=fcitx`、`XMODIFIERS=@im=fcitx`、
`XDG_CURRENT_DESKTOP=X-Cinnamon`、`XDG_SESSION_TYPE=tty`、`DISPLAY` / `WAYLAND_DISPLAY` 未設定。
既存のCinnamon / Fcitx5プロセスから指定項目だけを読み取ると `DISPLAY=:0`、
`WAYLAND_DISPLAY` 未設定だった。`GTK_THEME` / `GTK_PATH` / Fcitx固有のdirectory指定は未設定。
`XDG_SESSION_TYPE=tty` はdesktopプロセスにも継承されており、これだけでbackendを判定しない。
ホストの `org.cinnamon.desktop.interface` / `org.gnome.desktop.interface` の `gtk-theme` は双方 `Adwaita`。
`~/.config/gtk-4.0` は空で、`gtk.css` / `settings.ini` はなかった。

### 原因と影響の分類

**Fontconfig: 配布libraryとホスト設定の版の組合せ。**
同梱2.15.0の `FcInitLoadConfigAndFonts` だけで `xsi:nil` / `monospace` 等の警告を再現した。
`FcConfigGetConfigFiles` は `/etc/fonts/fonts.conf` とArchの `/etc/fonts/conf.d` を示す。
さらに[2.15.0の初期化処理](https://raw.githubusercontent.com/fontconfig/fontconfig/2.15.0/src/fcinit.c)は、
有効設定とは別にコンパイル時の `FC_TEMPLATEDIR`（`/usr/share/fontconfig/conf.avail`）も解析する。
新しいArch設定の属性・定数を古いparserが認識しない。isolated XDGでもシステム設定は残る。

ホスト2.18.3、同梱2.15.0＋ホスト設定、同梱2.15.0＋固定Ubuntu設定で
`FcFontMatch` を比較すると、日本語「日」「あ」はNoto Sans JP、U+1F600はNoto Color Emoji、
等幅LatinはNoto Sans Monoを選び、対象glyphの存在も確認できた。
この範囲ではfallbackの欠落は観測していないが、全フォント・描画・ユーザー設定の互換性は証明していない。

Ubuntu `/etc/fonts` の設定を一時領域へsymlinkを解決してコピーし、`FONTCONFIG_PATH` を
その領域だけに指定しても、ホスト `conf.avail` の解析警告は残った。
**設定の同梱と環境変数の追加だけで完全修正できるとは判断しない。**
[Fontconfigの設定仕様](https://fontconfig.pages.freedesktop.org/fontconfig/fontconfig-user.html)に従う場合も、
ホストフォント、XDGのユーザー設定とcache、template探索まで確認が必要。
設定を収録する場合はdpkg所有者と対応source / copyrightも収録対象となる。
DejaVu等の別package由来ルールを含める場合は、そのpackageの対応付けも必要になる。

ホスト2.18.3のlibraryを現在の同梱依存とともにロードする試験は成功したが、
Fontconfigをsystem dependencyへ移す全distribution互換性の証明にはならない。
既存のDebian 13検証環境には `libfontconfig1` 自体がなく、単純な除外はその環境を壊す。
この初回調査では設定・libraryを変更しなかった。その後の配布互換修正は下記参照。

**Fcitx: GTK4 module探索先の欠落で、入力機能へ影響する配布側の問題。**
Ubuntu GTKの `GTK_LIBDIR` は `/usr/lib/x86_64-linux-gnu` であり、GTK4の
`_gtk_get_module_path("immodules")` はその下と `GTK_PATH` を探索する。
Archの `/usr/lib/gtk-4.0` は元のAppRunでは探索先に入らない。
同梱GTK/GIOでホストmoduleを明示的にscanすると `fcitx` extension登録とtype classロードに成功した。
現在のGTK4 / GLibとホストmoduleの組合せについてABI解決を確認したもので、将来のホスト版を保証しない。

[upstreamの診断ツール](https://github.com/fcitx/fcitx5-gtk/blob/master/gtk2/immodule-probing.cpp)
`fcitx5-gtk4-immodule-probing` は表示接続とIM context生成だけを行い、windowを作らない。
同じ `DISPLAY=:0` / `GTK_IM_MODULE=fcitx` で比較した:

| 条件 | 実際のcontext | IM warning |
|---|---|---|
| ホストGTK | `fcitx` | なし |
| 同梱GTK（元の探索先） | `gtk-im-context-simple` | 再現 |
| 同梱GTK＋`GTK_PATH=/usr/lib/gtk-4.0` | `fcitx` | なし |

元の候補では指定されたFcitx経路が使われていない。context選択を実測できたため、
AppRunで存在するホストGTK4 directoryだけを既存 `GTK_PATH` の末尾に追加した。
`GTK_IM_MODULE`、theme、backend、XDG保存先は保持する。ホストmoduleやFcitxをコピー・同梱しない。
変更したAppRunは通常の対応ソース生成で収録・hash記録され、第三者binary/sourceの追加は生じない。

[FcitxのWayland資料](https://fcitx-im.org/wiki/Using_Fcitx_5_on_Wayland/en)は、
compositorのtext-input対応とGTK IM module経路を区別する。
GTK4のbuilt-in Wayland経路が使える環境もあるが、今回のsimpleへのfallbackが日本語入力を提供するとはいえない。
`XMODIFIERS` の設定だけでGTK4のFcitx module欠落を補えるとも扱わない。
無条件の `GTK_IM_MODULE` unsetやWayland/X11強制は行わない。
Fcitx修正候補では変換・確定・削除・フォーカス移動をユーザーが実機確認済み。

**Theme parser: Agnamのタグソースと同梱GTKのCSS構文の互換性問題。**
`src/app/mod.rs` の `APP_CSS` の3 / 4 / 8行はそれぞれ
`var(--sidebar-bg-color)`、`var(--sidebar-fg-color)`、`var(--sidebar-backdrop-color)` を使う。
その文字列だけを `GtkCssProvider` へ渡すと、同梱GTK 4.14.5で報告された位置の3警告を再現し、
ホストGTK 4.22.5では警告なし。表示初期化・host theme・ユーザーCSSはこの試験に関与しない。
isolated XDGでもタグソース内の同じ文字列が読み込まれる。
[GTKの公式CSS仕様](https://docs.gtk.org/gtk4/css-properties.html#custom-properties)では
custom properties / `var()` は4.16以降。libadwaitaの生成CSSが原因とする証拠はない。

3つの色宣言が受理されず、navigation railの背景・文字色・backdrop色が意図どおり指定されない。
GTKのfallbackで画面は表示されるが、dark/light時の色・contrastが適切かは別のGUI確認項目となる。
固定libadwaita 1.5.0の `src/stylesheet/defaults-{light,dark}.css` には対応する
`@sidebar_bg_color` / `@sidebar_fg_color` / `@sidebar_backdrop_color` があり、
この構文へ置換した一時文字列は同梱GTKで警告なくparseできた。
ただし正式ビルドは不変のv0.9.0タグarchiveを使うため、current Rustだけを直しても候補には反映されない。
タグ検証後の明示patch・対応ソースの記録、または次版sourceでの修正の判断が必要。
初回調査はここまでとした。その後、タグは不変のまま明示的な配布patchを追加した（下記参照）。

**mountinfo: ホストのutab lock権限と古いlibmount monitorの組合せ。無害とは未判定。**
通常ユーザーは `/proc/self/mountinfo` を読める。
同梱GIOで `g_unix_mount_monitor_get` / `g_volume_monitor_get` を呼ぶと、
AppImage runtimeもGTKの表示初期化も使わず同じ警告を再現した。sandbox外でも再現した。
`LIBMOUNT_DEBUG` 相当のmonitor診断ではmountinfoのopen / epoll登録は成功し、
userspace monitorで `-EACCES` になった。
ホスト `/run/mount/utab.lock` はroot所有0600で、一般ユーザーから読めない。

[libmount 2.39.3の処理](https://github.com/util-linux/util-linux/blob/v2.39.3/libmount/src/monitor.c)は
そのlockfileへのinotify登録を試し、`EACCES` の場合はdirectory監視へfallbackしない。
kernel monitor単独は成功、userspace単独と両者併用は `-13`。
[GLib 2.80の処理](https://raw.githubusercontent.com/GNOME/glib/2.80.0/gio/gunixmounts.c)は
libmount monitor全体の失敗を、固定のmtab path（mountinfo）を使って報告する。
したがって今回再現したエラーは、mountinfo自体の読み取り拒否を意味しない。

ホストlibmount 2.42.4では両者併用が成功する。
[2.42.4のuserspace monitor](https://github.com/util-linux/util-linux/blob/v2.42.4/libmount/src/monitor_utab.c)は
`utab.event` を監視し、ホストのそのfileは0644だった。
Type 2 / FUSE固有の失敗とする証拠はなく、古いupstream実装とホスト側file形式・権限の互換性に分類する。
同一原因を追跡するAppImage runtimeのupstream issueは特定していない。

mount一覧の初期取得は成功したが、同梱GLibのこの失敗分岐ではmount変更のwatcherが付かず、
この分岐からpollingへのfallbackもない。file chooser等の外部媒体一覧の自動更新に影響し得る。
初回調査時点では媒体操作は未確認だった。Fcitx修正候補では接続・取り外し時の一覧更新・媒体上fileの
読取りをユーザーが実機確認し、mountinfo警告も再現していない。今回の理由では追加対策しない。
`/proc` / utabの権限変更、system libraryの無検証コピー、monitorの無効化は行わない。

### Fontconfig / GTK 4.14の配布互換修正

固定Ubuntu snapshotの署名付きSources情報と新規downloadのSHA-256を照合し、
Fontconfig `2.15.0-1.1ubuntu2` の `.dsc` / orig / Ubuntu差分を追加の固定入力にした。
全Ubuntu quilt patch・hardening・RGB既定を維持して再構築し、公式Meson option
`-Dtemplate-dir=conf.avail` を指定する。新しいlibraryへのupgradeやソース改変は行わない。
[2.15.0のMeson規則](https://raw.githubusercontent.com/fontconfig/fontconfig/2.15.0/meson.build)では
明示した相対template-dirはprefixへ変換されず、`FcConfigFilename` が `FONTCONFIG_PATH` で解決する。
Ubuntuの有効な設定とtemplateをAppDirの `etc/fonts` へ原本のまま収録し、
AppRunで `FONTCONFIG_PATH` / `FONTCONFIG_FILE` を指定する。
空の `local.conf` はhostのsystem独自設定へのfallbackを防ぐ。Ubuntu原本と生成物を出所台帳で区別する。
Fontconfig libraryと設定のsource versionを照合し、DejaVu等の別packageの通知・対応ソースも記録する。
user独自ルールは `50-user.conf` によって引き続き読むため、2.15互換の構文が必要。
フォントの `/usr/share/fonts` / `/usr/local/share/fonts` / XDG user fonts / `~/.fonts` と通常のcacheは維持し、
fontそのものはAppImageへ収録しない。

v0.9.0のtag/commit/archive hashを確認した後、配布patchでnavigation railの3色だけを
`var(--sidebar-*-color)` → `@sidebar_*_color` へ変更する。元source、patch、適用後sourceを
SHA-256で照合し、fuzz/offset・二重適用・想定外sourceを拒否する。
対応ソースには未変更のtag archive、patch、適用処理、前後hashの `agnam-patches.json` が残る。
`validate.py source` はarchiveに収録した処理から差分を再適用し、payloadに抽出した実CSSとの一致を確認する。
current branchのRust source、tag、Cargo.lock、UI操作、theme、IME、XDG保存先は変更しない。

libadwaita 1.5.0の実resourceに定義された色は以下。themeを固定せず、この既存の名前を参照する。

| 色 | light | dark |
|---|---|---|
| `sidebar_bg_color` | `#ebebeb` | `#303030` |
| `sidebar_fg_color` | `rgba(0,0,0,0.8)` | `white` |
| `sidebar_backdrop_color` | `#f2f2f2` | `#2a2a2a` |

先行非GUI検査ではArch / 固定Ubuntu 24.04 / GTKなしDebian 13でFontconfig初期化と
patched CSS・light/dark既定色の `GtkCssProvider` 解析が成功し、対象警告は0件。
未修正の実タグCSSを同じGTK 4.14.5へ渡す負の試験は `Expected a valid color` で停止した。
host fontとXDG user fontの探索、JPEG/PNG/WebP/SVG decodeを確認した。
Archでは「日」「あ」・U+1F600・等幅Latinのmatch先とglyphの存在を確認した。
Ubuntu / Debianの検査用DejaVuでのpattern matchを、日本語glyphの表示確認とは扱わない。
検査用fontは出力領域の一時XDG directoryだけにコピーし、AppDirには入れない。
Bash構文、9 Python unittest、shellcheck、actionlint、currentとpatched tag sourceの
`cargo fmt --check` は成功。最終生成物の検査は以下の再生成結果に記録する。

最終候補のローカル再生成でも、Arch / Ubuntu 24.04 / GTKなしDebian 13で対象警告0件、
全102 ELFの解決・相対RPATH・JBIG/LZO除外、Type 2の展開・extract-and-runを確認した。
patchedタグsourceのrelease build / cargo check、archive 128件（既存ignore 2件）・thumbnail 140件（既存ignore 1件）も成功。
対応ソースの全13,072ファイルをhash検証し、archive内の処理からCSS patchと全native library / runtimeを再構築・再リンクした。
Fontconfigは初回buildとarchiveからの再buildでバイト単位でも一致した。
ネットワークを切り、外部recipeをmountしない別の一時環境でも、対応ソースarchiveから
全native library、Agnam本体の `cargo build --release --locked --offline` / `--help`、runtime再構築・version実行が成功した。
Rust compilerは検証済みの固定toolchainを読取り専用で使用し、元buildのobject / Cargo cacheは流用していない。
この再構築runtimeもSHA-256 `323205e3f90a07d51ad925a43773407c5047fdab4319e2d29aae5f742835a0db` で一致した。
Agnam本体の独立再buildは初回とhashが異なった。compiler配置等の条件も異なり、差の原因は確定していない。
全AppImageのビット単位の再現性保証とは区別する。
Docker / Podmanがないため、既存の固定Ubuntu rootfsをuser / mount / PID / network namespaceで隔離して検証した。
tarの所有者復元とAPTの取得UIDにはこの一時環境だけで調整が必要だったが、配布scriptの権限や署名/hash検証は変更していない。
このローカル検証はActions結果とは区別する。Fontconfig / CSS修正後の最終run `37574815351` の成功結果は上記参照。

ローカル検証済み成果物（`target/appimage-fontconfig-css/dist/`、Actions成果物ではない）:

| 成果物 | bytes | SHA-256 |
|---|---:|---|
| `Agnam-0.9.0-x86_64.AppImage` | 39,537,024 | `f9b1f58993bd3e861674034f7c01653ce260aa3497e53c314da0fca00c7663dd` |
| `Agnam-0.9.0-AppImage-corresponding-source.tar.xz` | 566,934,172 | `48fe24f2bcf2dd1f5605af1ebc5e229e7122c7112f214013e93bf88f50dd0a98` |

その後の最終Actions候補で、起動stderrのFontconfig / Theme parser warning、日本語IME、日本語/emoji、
light/dark・railのactive/non-active、file chooserをユーザーが再確認した（上記参照）。
RARのsingle entry safety limit拒否は既存の安全仕様であり、この修正の対象外。

### 初回警告調査の比較・修正検証と実機確認手順

候補そのものの通常実行、`--appimage-extract-and-run`、展開後 `AppRun` は、
すべて `--help` がexit 0だった。sandbox内の通常実行のみFUSEが見えず失敗したため、
通常のhost環境で同じ `--help` を再実行して確認した。GUIは起動していない。
helpはGTK/volume monitorを初期化しないため、警告がないことをGUIの互換性判定に使わない。
3方式での通常GUIの比較は実施していない。その後の最終候補の通常起動で警告解消と操作を確認済みで、
起動方式依存の問題を示す証拠はないため、その追加比較を採用条件とはしない。

取得済み候補の全102 ELF、system library境界、JBIG/LZO除外、相対RPATH、依存解決、
desktop / icon / noticeを `validate.py payload` で再確認した。
既存の非GUIprobeでJPEG/PNG/WebPの32×16、SVGの128×128 decodeと同梱libraryの初期化を確認した。
候補から作業用AppDirを別にコピーして新AppRunだけを入れ、差分がAppRunだけであることを照合した。
そのAppDirでも同じpayload検証とdecodeは成功。正式候補のAppImage再生成には相当しない。
Bash構文、8 Python unittest、shellcheck、actionlintは成功。
testは既存/未設定のGTK_PATH、GTK_IM_MODULE / GTK_THEME、displayとXDG保存先、引数の保持を確認する。
別の診断用AppDirでAgnam executableだけをwindowを作らないIM診断ツールへ置き換え、
新AppRunをそのまま実行すると `fcitx` contextを選び、IM warningは出なかった。
この診断用AppDirを配布候補として扱わず、日本語の入力操作まで検証したとは扱わない。
測定スクリプト・出力・展開物はGit管理対象外の `target/appimage-warning-audit/` に保存した。

再現の要点（repository root、`appdir` は取得候補を展開した絶対path）:

```sh
appdir=/absolute/path/squashfs-root
LD_LIBRARY_PATH="$appdir/usr/lib" python3 - <<'PY'
import ctypes as c
from pathlib import Path
import re

f = c.CDLL('libfontconfig.so.1')
f.FcInitLoadConfigAndFonts.restype = c.c_void_p
print('Fontconfig:', f.FcGetVersion(), bool(f.FcInitLoadConfigAndFonts()))

g = c.CDLL('libgtk-4.so.1')
g.gtk_css_provider_new.restype = c.c_void_p
g.gtk_css_provider_load_from_data.argtypes = [c.c_void_p, c.c_char_p, c.c_ssize_t]
css = re.search(r'const APP_CSS: &str = r#"(.*?)"#;', Path('src/app/mod.rs').read_text(), re.S).group(1).encode()
g.gtk_css_provider_load_from_data(g.gtk_css_provider_new(), css, len(css))

gio = c.CDLL('libgio-2.0.so.0')
gio.g_unix_mount_monitor_get.restype = c.c_void_p
print('mountinfo bytes:', len(Path('/proc/self/mountinfo').read_bytes()))
print('mount monitor:', bool(gio.g_unix_mount_monitor_get()))
PY
```

Fcitx診断ツールが存在するこのArch環境では、GUIセッションのterminalから
`fcitx5-gtk4-immodule-probing`、
`LD_LIBRARY_PATH="$appdir/usr/lib" fcitx5-gtk4-immodule-probing`、
`LD_LIBRARY_PATH="$appdir/usr/lib" GTK_PATH=/usr/lib/gtk-4.0 fcitx5-gtk4-immodule-probing`
を比較できる。これは日本語入力操作の確認ではない。

初回調査で提示した再確認手順は以下。その後の確認済み範囲は上記の最終採用判断に記録した:

1. 実際のsession環境を保持して起動し、本棚検索欄で「にほんご」を入力、変換候補を選び、
   「日本語」を確定する。削除・フォーカス移動・file chooserの入力も確認する。
   IM moduleを使うX11と、普段利用するWayland環境は別に判定する。
2. 日本語label・文字を含むBook名・emojiの欠落、等幅表示、dark/lightと非active時のnavigation railの色・contrastを確認する。
3. file chooserを開いた状態で通常のdesktop操作により外部媒体を接続し、一覧更新・選択・読取りを確認する。
   取り外し後の一覧更新も確認する。権限やmount設定を変更して警告を回避しない。
4. 起動方式依存が疑われる場合だけ、終了してから通常AppImage、extract-and-run、展開AppRunを
   順に起動し、同じ操作とstderrを比較する。環境変数をunsetして比較結果を混同しない。

今回のRAR thumbnailエラーは既存single entry 64 MiB安全制限であり、この4警告とは別。
制限は変更しない。その後の修正・最終Actions・実機確認によってIssue #1の採用条件を満たした。

## 一次資料

- [Issue #1](https://github.com/nagochiCC/agnam/issues/1)
- [type2-runtimeの固定Makefile](https://raw.githubusercontent.com/AppImage/type2-runtime/8f39b89e2ac31e1640b3d3f7e9a5108e6ce805fa/src/runtime/Makefile)
- [上流libfuse patch](https://github.com/AppImage/type2-runtime/blob/8f39b89e2ac31e1640b3d3f7e9a5108e6ce805fa/patches/libfuse/mount.c.diff)
- [libjpeg-turbo 3.2.0の原license / IJG条件](https://github.com/libjpeg-turbo/libjpeg-turbo/blob/3.2.0/LICENSE.md)
- [TIFFのbuild資料](https://libtiff.gitlab.io/libtiff/build.html)。実際の4.5.1 optionは取得sourceの `cmake/JBIGCodec.cmake` を照合した。
- [CC-BY-SA 4.0原文](https://creativecommons.org/licenses/by-sa/4.0/legalcode.txt)
- Ubuntu・Cargo・Rust・nativeの実原文と対応表は生成物内に保持する。

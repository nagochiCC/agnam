# 開発版AppImageの生成と取得

Linux x86_64向けの開発版は **AppImage development (Artifacts)**
（[workflow](../../.github/workflows/appimage-development.yml)）で生成する。
正式版の `v0.9.0` 固定ソースを使う [既存workflow](../../.github/workflows/appimage.yml) とは別経路である。
Release / Pre-release、version tagを作らず、既存Release assetsも更新しない。
Cargo.tomlの正式versionは変更せず、現在のアプリ内部のversion表示は `0.9.0` のままとなる。
開発版はファイル名、ビルド元commit SHA、Actions run ID / attemptで識別する。

## GitHub上で実行する

1. このworkflowとpackaging実装を `develop` へ統合する。現在のdefault branchは `develop`。
   `workflow_dispatch` はworkflowがdefault branchに存在することが前提である。
   default branchが変更された場合も、同じworkflowをそのdefault branchへ反映する必要がある。
   [GitHubの手動実行仕様](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/manually-run-a-workflow)も参照する。
2. repositoryの **Actions → AppImage development (Artifacts) → Run workflow** を開く。
3. Branchを **develop** にして実行する。実行にはrepositoryへのwrite権限が必要。
   他branch、tag、fork上の実行はbuild jobをskipする。任意refや外部repositoryの入力欄はない。
4. runの全工程が成功したことを確認する。run summaryの **Development source commit** と
   checkoutしたSHA、Artifact内の `source-authority.json` の `commit` が一致することを確認する。
5. runの **Artifacts** から
   `Agnam-development-x86_64-<40桁SHA>-<run ID>-<attempt>` をダウンロードする。
   同じcommitの再実行もrun ID / attemptで区別できる。

Artifactの保持期間は原則 **30日**。repository側の保持設定や手動削除により早く利用できなくなる場合がある。
取得にはGitHubへのログインとrepositoryへのread権限が必要となる。
ダウンロード手順は [GitHubのArtifact取得仕様](https://docs.github.com/en/actions/how-tos/manage-workflow-runs/download-workflow-artifacts) を参照する。
失敗時の `Agnam-development-failed-build-diagnostics-...` は診断用で、配布用AppImageではない。

## 取得したファイルを確認する

ArtifactのZIPを展開すると次のものが入る:

```text
Agnam-development-x86_64.AppImage
Agnam-development-x86_64.AppImage.sha256
Agnam-development-AppImage-corresponding-source.tar.xz
Agnam-development-AppImage-corresponding-source.tar.xz.sha256
source-authority.json
development-build.json
binary-provenance.json
file-manifest.json
ubuntu-packages.json
validation/
```

同じdirectoryで両方のchecksumを確認してから実行権限を付ける:

```sh
sha256sum -c Agnam-development-x86_64.AppImage.sha256
sha256sum -c Agnam-development-AppImage-corresponding-source.tar.xz.sha256
chmod +x Agnam-development-x86_64.AppImage
./Agnam-development-x86_64.AppImage
```

checksumは取得したファイルの破損確認用で、独立した署名ではない。
ファイル名だけではビルドを区別できないため、取得元のrun URLとcommit SHAも保存する。
`source-authority.json` にはrun URL / ID / attempt、workflow commit、ソースarchiveのhashと
commitのファイル一覧を記録する。`development-build.json` にはアプリversion、Cargo.lock、
実際にコンパイルした各ソースと収録実行ファイルのhashを記録する。
同じ記録はAppImage内の `usr/share/doc/agnam/` と対応ソースにも入る。

## ビルド入力と検証

`development.py snapshot` はrunの `github.sha` とcheckoutのHEADを照合し、
tracked fileの変更を拒否する。固定SHAへの `git archive` とcommit treeの全blob / mode、
checkout内容を比較する。submoduleやexport-ignore / export-substによる内容欠落・改変は拒否する。
ソースは `agnam-<SHA>.tar.gz` に凍結し、以後branchの更新やタグarchiveを参照しない。
container内でもarchive checksum、manifest、マウントしたrecipeとcommitの一致を確認する。
checkoutの `.cargo` 設定は依存取得先を変更させないため拒否する。

Ubuntu 24.04の固定digest / 署名付きAPT snapshot、Rust toolchainとnative入力のchecksumは
[`inputs.json`](inputs.json) / [`Dockerfile`](Dockerfile) の正式版と同じ固定値を利用する。
開発版はその中の `agnam_commit` と `sources.agnam` を使用しない。
`cargo vendor --locked` の全package / fileを **開発版のCargo.lock** と照合し、
`cargo build --release --locked --offline` でそのソースをコンパイルする。
archive / thumbnail testとcargo checkも実行する。

現在のnavigation railの3色にはGTK 4.14互換の置換が必要である。
開発版専用の処理は `APP_CSS` 内の `var(--sidebar-*-color)` を `@sidebar_*_color` へ置換し、
変更前後hashと置換一覧を `agnam-patches.json` へ記録する。
すでに互換色を使う場合は置換を行わず、未知の色定義や未対応CSS変数は失敗させる。
`v0.9.0` 専用patchやその固定ソースhashは適用しない。repository本体のCSSも書き換えない。
実際にコンパイルしたCSSを抽出し、同梱GTKのparserでlight / darkと警告を確認する。

既存 `validate.py` のAppImage展開内容・全ELF・native依存・host library境界・
非GUI probe・通知・対応ソースmanifest・native library/runtime再ビルド・再リンクを維持する。
開発版では対応ソースからCSS置換を再現し、vendorのcoverageとコンパイルしたソースの一致を確認し、
アプリもfreshなtargetへオフライン再ビルドする。
別containerでnetworkを無効化したruntime再ビルドとDebian 13での非GUI検証も成功した場合だけ
正常Artifactをアップロードする。ソース生成・検証に失敗したAppImageは配布成功扱いにしない。
bit単位の再現性やGUI動作の成功をこれらの検査から保証するものではない。

## ライセンスと対応ソース

正式版と同じ通知収集を使い、GTK / libadwaita等の実収録Ubuntu packageのcopyrightと対応source、
Rust全lockfileの再帰的LICENSE / NOTICE、Rust標準library、native library、AppImage runtimeと
libfuse patchを保持する。`unrar_sys = 0.5.8` のUnRAR 7.01 source、独自license、acknow.txt、
Intel CRC32 / BLAKE2spの帰属もvendorと通知に含む。
UnRARにはRAR / WinRAR互換圧縮器の開発禁止条件がある。
原文の入口はAppImage内 `usr/share/doc/agnam/THIRD_PARTY_NOTICES.md`、`SOURCE_CODE.md` と
`third-party/`。自動収集だけで法的な完全性を判定せず、実生成物の通知とinventoryを確認する。

対応ソースarchiveは固定SHAの未変更Agnamソース、Cargo vendor、Rust標準library source、
native / runtime入力、Ubuntu source一式、patch、実build設定とprovenance、recipe、全fileの
`SHA256SUMS` を含む。AppImageと対応ソースは同じArtifactで一緒に提供する。
再配布時もこの組を保持する。30日以降の恒久配布経路はこのworkflowの対象外である。

再ビルド・再リンクの前提環境とlibraryの差し替え方法は
[`README.md`](README.md#通知と対応ソース) の共通native / runtime手順を使う。
同じ固定Ubuntu container内で対応ソースを `/build/corresponding-source` へ展開し、
`sha256sum -c SHA256SUMS` を確認する。
Agnam側は **正式版patch適用手順の代わりに** 次の準備を行う:

```python
import json, sys
from pathlib import Path
sys.path.insert(0, '/build/corresponding-source/packaging/appimage')
import development, package
s = Path('/build/corresponding-source')
authority = json.loads((s / 'source-authority.json').read_text())
archive = development.verify_archive(s / 'inputs', authority)
app = package.extract(archive, Path('/build/agnam-rebuild'))
development.verify_tree(app, authority['files'])
assert development.apply_css(app) == json.loads((s / 'agnam-patches.json').read_text())
package.verify_vendor(app, s / 'vendor')
(app / '.cargo').mkdir()
(app / '.cargo/config.toml').write_text(
    '[source.crates-io]\nreplace-with="vendored-sources"\n'
    '[source.vendored-sources]\ndirectory=' + json.dumps(str(s / 'vendor')) + '\n')
print(app)
```

`inputs.json` に固定したRust toolchainを元のchecksumで検証して導入し、
`native.sh ... all` の再構築prefixと [`build-development.sh`](build-development.sh) の環境設定で
`cargo build --release --locked --offline` を実行する。
source準備とlibrary差し替えに必要な情報はarchive内にも同梱する。

## 現在の検証範囲

開発版workflowの実装とローカルのscript / authority / Rust関連検証を備える。
scriptの検証は次の手順で実行できる:

```sh
bash -n packaging/appimage/build.sh packaging/appimage/build-development.sh packaging/appimage/native.sh packaging/appimage/AppRun
python3 -m unittest discover -s packaging/appimage -p 'test_*.py'
# actionlintを利用できる環境:
actionlint .github/workflows/appimage.yml .github/workflows/appimage-development.yml
```

GitHub Actions上の実AppImage生成、Artifact取得と取得物のchecksum、実通知一式、GUI動作は未確認。
実際の成功runやダウンロードリンクはまだない。配布完了とは扱わない。
[Issue #3](https://github.com/nagochiCC/agnam/issues/3) はActions成功・取得確認と必要な実機確認、
明示的な統合指示までopenで追跡する。

生成後はGTK / Fontconfig警告、日本語IME、light / dark、file chooser、画像と
ZIP / RAR / 7zの読取り、本棚 / thumbnail、設定・履歴の再起動後復元を実機で確認する。
正式版のGUI確認済み結果を開発版へそのまま適用しない。
動作環境は正式版と同じglibc 2.39以上とhostの表示 / GPU libraryを必要とする。

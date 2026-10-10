# Agnam current status

AgnamはRust + GTK4 / libadwaitaで実装している漫画・画像Viewerである。この文書はcurrent implementation / verification stateを短時間で把握するためのsnapshotであり、changelog、作業log、backlogではない。詳細仕様は [`README.md`](README.md) から該当する `design/` documentを参照する。

## Current implementation

- ViewerはSingle / Spread表示、通常page移動、単page移動、指定page移動、fullscreen、HeaderBar / Sliderの自動非表示、Document境界page、smart cropに対応している。
- ViewerはTexture / Paintable cacheと圧縮画像Byte cacheを分離し、background preload、hover thumbnailの32 MiB memory cacheと1 GiB soft limitの永続disk cache、stale非同期結果の排除を行う。smart cropは別workerの軽量なcrop解析結果をcacheし、通常GDK Textureへ描画時cropを適用する。Spreadの左右は物理中央で分割しsource Textureを共有する。
- Libraryはfilesystem本棚、任意階層navigation、画像Book、container / seriesフォルダ、archive内容表示、breadcrumb、Cover / Folder preview、並び替え、読書進捗に対応している。
- 通常filesystem本棚はsession scan cacheとversion付きroot snapshotを使い、archiveのviewable-content probe、Cover生成結果、decoded Textureも用途に応じてcacheする。
- 画像Book / archive Bookは手動Cover overrideに対応し、`cover.xxx` は通常読書pageから除外する表紙専用画像として扱う。明示的にそのfile自体を開いた場合は単独画像として閲覧できる。
- 左navigation railと共通drawerから検索、履歴、お気に入りを利用できる。各panelはdrawer closeやpanel切替で保持状態を失わない。
- 検索はmemory index、履歴は作品単位identity、お気に入りはDocument単位identityを使用し、それぞれCoverの遅延取得・再利用を行う。
- 設定、通常window状態、last Document / logical pageを永続化し、起動時に復元できる。
- Library / Viewer共通Drag and Dropに対応している。
- archiveはZIP / CBZ、RAR / CBR、7z / CB7、TAR / CBT、LZH / LHAとnested archiveに対応する。RAR / 7zは条件に応じて全展開完了前から閲覧できるprogressive loadを使用する。
- archive operationの累積read / 展開・temporary disk累積write / 同時occupancyは最大展開データ量設定（2 / 4 / 8 / 16 GiB、default 4 GiB）の開始時snapshotを使う。画像・通常fileのentry上限は64 MiB、nestedとしてdisk materializeするarchive entryだけは同じ設定値を上限にする。RARの `RAR_TEST` callback / 他形式のbounded readerを使い、Viewer・archive contents・Coverは内部archive全体をVecに保持せず、共有budget・cancel・workspace lifetimeを維持する。total / image entries各4,096件とSequentialの256 MiB spill thresholdは維持する。
- 画像はPNG、JPG / JPEG、WebPに対応する。

## Architecture map

- `src/app/`: GTK4 / libadwaitaのAppとUI統合、navigation、Library / Viewer / drawer、load requestの調停。
- `src/viewer/`: `ViewerSession`、表示state、Texture / Byte cache、background preload / thumbnail scheduler、smart crop preparation、decode / render。
- `src/document/`: `Document`、`ImageAsset`、`Page`、`AssetId` 等の表示data model。
- `src/archive/`: archive判定、読込、access strategy、nested archive、安全な展開、前後file探索。形式固有処理は `archive::formats`。
- `src/bookshelf/`: GTK/App非依存の本棚scan、検索index、scan snapshot、archive probe cache、Cover生成・disk cache。
- `src/thumbnail/`: Viewer Slider用thumbnail生成。
- `src/history/`: GTK非依存の作品単位履歴modelと永続化。
- `src/favorites.rs`: GTK非依存のお気に入りidentityと永続化。
- `src/settings.rs`: 永続`UserSettings`とlast session。

## Important current constraints

- 永続設定はAppが所有する `settings::UserSettings` を信頼できる状態とし、設定画面やcontrollerへ同じ設定値を重複所有しない。
- `ViewerSession` が現在の `Document`、`ViewState`、Viewer cache、`document_generation` を所有し、AppがDocument内部dataを重複所有しない。
- UI signal / Action / background結果は `AppSender` から共通 `Msg` queueへ送り、GTK main context上の `App::dispatch()` → `sync_ui_state()` で処理する。request / generationの意味を混同せず、stale結果を現在UIへ反映しない。
- 物理画像と論理pageは `ImageAsset` / `Page` で分離し、同一画像判定には `AssetId` を使う。Spread境界を単純なpage番号の偶奇だけで判断しない。
- 本棚root直下のBookは独立したタイトルとして扱い、通常page移動・前後file移動のどちらでも別Bookへ自動遷移しない。通常フォルダ内では既存のsibling Document移動を維持する。
- archive formatとaccess strategyを分離し、形式固有処理をUIへ漏らさない。nested archiveの再帰上限、安全な一時展開、stale / cancel処理を維持する。
- 既存UI、操作体系、accelerator、設定互換性は明示的な仕様変更なしに変更しない。

## Verification / known constraints

- v0.9.0の固定ソースからUbuntu 24.04基準のAppImageと対応ソースを生成する `packaging/appimage/` とActions workflowを備える。最終候補のActions全工程、Debian 13非GUI検証、対応ソースからのlibrary / runtime再ビルド・再リンク、通知一式の整合確認が完了した。Arch実機で日本語IME・表示・file chooserと起動警告の解消を確認済み。外部媒体操作は前候補で確認済み。Issue #1の配布可否調査を完了し、AppImage・checksum・対応ソースを [GitHub Release v0.9.0](https://github.com/nagochiCC/agnam/releases/tag/v0.9.0) で公開済み（[最終検証記録](research/issue-1-appimage-release-build.md#最終actions候補の採用判断)）。

- checkoutしたdevelopの正確なcommitから開発版AppImage・checksum・対応ソース・provenanceを生成し、全検証成功後に30日保持のActions Artifactsへ提供する別workflowを備える。正式版の固定authorityは維持する。開発版のscript / source authorityと関連Rust testはローカル検証済み。実Actionsビルド、Artifact取得・取得物checksum、通知とGUIの確認は [Issue #3](https://github.com/nagochiCC/agnam/issues/3) で追跡する（[生成・取得手順](../packaging/appimage/DEVELOPMENT.md)）。
- 個々の実装・GUI確認・検証runはGitHub Issuesで追跡し、current behaviorに影響する確定結果だけdocsへ反映する。
- archiveの容量境界、65 MiBのRAR entry、RAR4 / RAR5・solid依存、対応形式のnested、cancel / I/O注入・cleanupと非同期回帰をRust testで確認している。decoder内部のRSS / CPU上限と即時cancelは保証対象外。実disk full、設定UI、実大容量RARの閲覧とGUI回帰確認は [Issue #2](https://github.com/nagochiCC/agnam/issues/2) の実機確認事項である。
- nested archiveには再帰上限があり、安全な処理のため一時directoryを使う場合がある。
- JPEG thumbnailはTurboJPEG / libjpeg-turboを使用し、現在のbuild構成では `pkg-config` から検出できる `libturbojpeg` とSIMD supportを前提とする。
- PNG / WebPのCover thumbnailはJPEGの縮小decodeより生成costが高く、大量画像を含むarchiveではthumbnail生成に時間がかかる場合がある。CPU負荷抑制のため高並列decodeは行わない。
- ViewerのPNG / WebP thumbnailとsmart crop解析は、処理中に所有されているdecoded imageだけをWeak経由で共有する。解析pixelsをViewer backingや長寿命cacheにせず、JPEG縮小thumbnail / thumbnail disk cache hitからcropを生成しない。smart crop toggleやTexture再生成ではcrop-result cacheを再利用する。
- `gtk-enable-animations=false` 環境では `AdwOverlaySplitView` のscrim反映にallocation補正が必要で、drawer open後のidleで `queue_allocate()` を行う既存workaroundを維持する。

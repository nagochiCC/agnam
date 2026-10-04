# Archive段階的読み込み仕様

この文書は、非nested RAR / CBRおよび7z / CB7を全展開完了前から閲覧可能にする段階的読み込みの安定した仕様・設計判断をまとめる。

## 対象範囲

- 対象は通常の非nested RAR / CBR、7z / CB7。
- ZIP / CBZ、TAR / CBT、LZH / LHA、nested archiveの既存load方式は変更しない。
- `InitialPage::End` は全体のpage構成確定が必要なためprogressive対象外とし、従来の一括読み込み経路を使う。
- 目的はtime-to-first-viewを短縮することであり、画像backingはmemoryから256 MiB spill threshold超過時にDocument所有の一時directoryへ移る。256 MiBはhard limitではない。

## 共通progressive基盤

- format固有extractorは `ProgressiveArchiveImage` として自然順上の物理画像index、画像byte、`ImageLayout`をAppへ通知する。
- App側のbuffer、contiguous prefix、pending page move、stale結果排除、Loading / Slider制御は `ProgressiveArchiveState` でformat非依存に管理する。
- Viewer側は `ViewerSession` のprogressive APIを共通利用し、RAR用・7z用の別stateを作らない。
- format固有のentry identity、展開API、error処理は `archive::formats` の各実装へ閉じ込める。
- 1 progressive load全体でsingle entry 64 MiB、累積read / 展開512 MiB、temporary disk累積write 512 MiB・同時occupancy 512 MiB、total / image entries各4,096件のoperation budgetを共有する。nested archiveも同じbudgetをdescendantまで共有する。

## 展開順と自然順

- archive内部のentry / decode順と漫画のnatural sort順は別物として扱う。
- 展開Workerは、そのDocumentが閲覧対象である間は内部順で最後まで処理を継続する。別Documentへ移動する、本棚へ戻る等で不要になった場合はcancel tokenによる協調キャンセルを要求し、安全なentry / block境界で早期終了する。
- キャンセル要求時に1entryのdecode中であれば、そのentryの処理が戻るまで停止が遅れることは許容し、強制的なthread停止は行わない。
- 各画像の展開完了時に、自然順上の物理画像index、画像byte、`ImageLayout`を途中結果として通知する。
- 自然順の先頭から連続して揃った画像だけをViewerへ解放する。
- 先行して取得した後続画像は捨てずに保持し、途中の欠けが埋まった時点で連続範囲をまとめて拡張する。
- 同名entryが存在しても名前だけをidentityにせず、archive内のstableなentry identityで区別する。

例: 内部順が `001, 002, 004, 005, ..., 003` の場合、001と002は閲覧可能にするが、004以降は003取得までViewerへ解放しない。003取得後に既取得の004以降も続けて利用可能にする。

## RAR固有処理

- RARは `unrar` のlisting順に対応するarchive indexをentry identityとして使う。
- RARは `unpacked_size` をfull allocation前に64 MiBと照合し、`header.read()`後のactual byte lengthも検査してoperation budgetへ加算する。current `unrar 0.5.8` APIではdecode途中のruntime byte stopを呼出側から強制できず、metadataの過小申告まで含むallocation前hard limitを完全保証しない。
- listing結果とprocessing結果の対応を確認し、同名entryでも別entryとして扱う。
- 画像の展開完了後に共通 `ProgressiveArchiveImage` へ変換してAppへ通知する。
- progressiveキャンセルはlisting中、entry処理前、画像entryの展開完了後などの安全な境界で確認する。`header.read()` 実行中のentryを強制中断しない。

## 7z固有処理とsolid制約

- 7zは `sevenz-rust` のmetadata上のfile indexをentry identityとして扱い、natural sort上のphysical indexとは分離する。
- progressive時は `BlockDecoder` をfolder順に駆動し、画像entryはreaderを最後まで読み込んで保持・通知する。これによりcallbackの継続可否をarchive全体へ伝播し、cancel時に後続folderまで不要に巡回しない。
- solid 7zでは後続entryをdecodeするために同じsolid block内の前方streamを消費する必要がある。表示対象外の通常fileもbytesは保持せずreaderを最後までdrainする。ただしarchive自体がcancelされた後は後続entryを必要としないため、安全なentry / block境界で終了する。
- directoryやstreamを持たないentryでは不要なdata読み込みを行わない。
- natural sort上の先頭画像がsolid block内の後方にある場合、そこへ到達するまで前方streamのdecodeが必要なため、先行表示開始が遅くなることがある。これはarchive形式上の制約として許容する。
- solid archiveを速くするための独自並列展開やblock再配置はこの機能の責務に含めない。

## Viewer先行表示

- 最初の有効な表示単位が揃った時点で、archive全展開完了を待たずViewer表示を開始する。
- Single / Spread、先頭表紙、横長AssetのRight / Left、`spread_shift`等の既存表示規則を維持する。
- Spreadでは、未完成の見開きを一時的な単pageとして表示して後から組み替えることは避け、現在の表示単位として成立するだけの連続pageが揃ってから利用可能にする。
- progressive中に画像が増えるたび全state / cacheをresetせず、progressive専用stateで連続範囲を拡張し、完了時に正式Documentへ移行する。
- 256 MiB spill後もhard limitまで読み込みを続ける。resource limit到達時はprogressive loadを正常完了扱いにせず、途中表示はcurrent failure semanticsに従い、正式Historyを記録しない。

## ページ移動待ち

- 取得済みの連続範囲内では通常どおりViewer内ページ移動できる。
- 次の表示に必要な自然順画像が未取得の場合、その移動を1件だけpendingとして保持する。
- pending中はViewer内のページ移動を無効化し、追加のページ移動要求を積まない。
- 対象画像が利用可能になったら、ユーザーの再操作なしでpending移動を1回だけ成立させる。
- pending中でも、本棚へ戻る、別Documentを開く、ウィンドウを閉じる等のViewer外操作は可能とする。
- 現在のDocumentを離れた場合はpendingを破棄してprogressive Workerへcancelを通知し、古い`load_request_id`の途中結果・完了結果も現在UIへ反映しない。

## Loading表示

- 最初の表示可能画像が出るまでは、共通の200ms遅延付き中央Loading Overlayを使用する。
- Viewer先行表示後は、archive全体のbackground展開が続いていることだけを理由にLoadingを表示しない。
- 未展開targetへの移動がpendingになった場合、その待機が200msを超えたときだけ共通の中央Loading Overlayを表示し、target到着後に移動して解除する。
- progressive専用の下部reserved領域やbackground展開中表示は設けず、通常loadと同じLoading Overlayを使う。

## Sliderとthumbnail

- progressive表示中も通常Sliderの配置とauto-hide規則を維持する。Sliderのrange / valueはnatural sort後の物理画像index / 総物理画像数を使い、全展開完了後にlogical page基準へ戻す。
- Sliderで指定した物理画像がすでに展開済みなら即時移動する。未展開targetは1件だけpendingにし、順次展開で利用可能になった時点で移動する。
- progressive中は通常Document向けの全域background ThumbnailWorkerを開始しない。hover対象のAssetがすでに展開済みなら、保持済みMemory bytesまたはtemp fileからbackgroundでオンデマンド生成できる。thumbnail目的のarchive再open / 再展開や未展開Assetへの先行処理は行わない。
- 全展開完了後は通常のbackground ThumbnailWorkerへ切り替える。Slider配置・preview選択の詳細は [`ui.md`](ui.md) と [`viewer.md`](viewer.md) を参照する。

## 完了・失敗・履歴

- archive全体の展開が正常完了したら、progressive表示中の現在位置とViewModeを可能な限り維持したまま正式Documentへ移行する。
- 完了時に先頭pageへ戻したり、同じDocumentを再度開いたようなToastや画面遷移を発生させない。
- 不要なcache全消去や再decodeを避ける。
- 閲覧履歴への正式記録と通常thumbnail生成は、archive全体の正常完了後に行う。
- 先行表示後に残りの展開が失敗した場合、部分状態を完成Documentとして扱わない。pending、Loading表示、stale結果を安全に終了・排除する。
- ユーザー操作による協調キャンセルは通常の読込失敗とは区別し、error表示やfallback処理を発生させない。
- resource-limit errorはcancelやarchive破損・filesystem errorとは専用kindで区別し、安全上限へ到達して読み込みを止めた旨を現在のwindow上の日本語dialogで表示する。pending page move、Loading、stale結果はfailure pathで終了する。
- 途中通知と最終Documentのmemory sourceは同じ `glib::Bytes` を共有し、画像全体の不要なcopyを増やさない。

## 世代管理

- progressive途中通知はload要求の`load_request_id`と結び付ける。
- `load_request_id`、Viewerの`document_generation`、thumbnail generationは別目的の世代として維持する。
- cancel tokenは不要になったWorkerの計算を止めるための補完であり、`load_request_id`によるstale結果排除を置き換えない。
- 別Documentへ移動した後に古いWorkerの途中通知・完了通知が現在Viewer、cache、履歴、UI stateを変更しないことを保証する。

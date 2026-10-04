# Viewer設計仕様

この文書はViewer、Document/Pageモデル、見開き、cache、thumbnail、archive、非同期loadに関する安定した仕様・設計判断をまとめる。関連作業で必要な箇所だけ参照する。

## Documentとページモデル

- `Document` は現在の表示対象として読込元 `path`、物理画像 `ImageAsset`、論理表示ページ `Page`、必要に応じた `temp_dir` を所有する。
- `ViewerSession` が `Option<Document>` を所有し、AppがDocument pathや一時directoryを別途複製して所有しない。
- `ImageAsset` は物理画像1枚を表し、`ImageSource`、`first_page`、`ImageLayout` を持つ。`ImageSource` は `File` / `ArchiveEntry` / `Memory` とし、archive形式固有の詳細をUI側へ漏らさない。`ArchiveEntry` はarchive pathに加えてstableなentry indexと期待entry名を保持し、同名entryも区別する。
- `Page` は論理ページで、元Assetを示す `AssetId` と `PagePart` を持つ。通常画像は `Whole` 1ページ、横長画像は同じAssetから `Right` / `Left` の順に2ページを構築する。
- 同一物理画像かどうかをPathやImageSourceの比較から推測せず、`AssetId` を使う。

## 表示モード・見開き

- 表示モードはSingle / Spread。`ViewState` が表示単位を管理する。
- Spreadでは右綴じの自然な見開きを維持するため `spread_shift` で境界を明示的に管理し、単純なページ番号の偶奇だけで推測しない。
- `view_starts` は表示単位の開始位置を保持し、ページ移動・指定ページ移動では表示モードに応じて適切な表示単位へsnapする。
- Single / Spreadの選択は永続化するが `spread_shift` はDocument閲覧中だけのruntime状態で永続化しない。
- 復元logical pageは現在の表示モードとページ数に対して既存snap規則で安全な表示単位へ収める。
- UI表示用のSlider callback dataやページlabel等を `ViewState` へ持ち込まない。

## Viewer cacheとpreload

- ViewerはTexture cacheとByte cacheの2層を使用する。
- Texture cacheは論理page indexをkeyに `GdkPaintable` を保持し、前・現在・次の表示単位を対象とする。通常SingleはGDK Textureそのもの、crop済みSingleとSpreadの各halfはsource Textureと表示矩形を持つ `PagePaintable` を使う。Spreadの片側だけを独立に破棄しない。
- Byte cacheは物理画像 `AssetId` をkeyに圧縮画像byte (`glib::Bytes`) を保持し、同じ横長画像のRight / Leftで共有する。
- Byte preloadは次の2表示単位、Texture preloadは次の1表示単位を基本とする。TextureはByteより狭い範囲で保持する。
- Texture保持判定はSingle / Spreadと `spread_shift` を反映した表示単位、Byte保持判定はAssetが占めるlogical page範囲からの距離で行う。
- 現在表示のcache missでは同期読込/decode経路をfallbackとして維持する。Byte preloadはbackground I/Oで次の2表示単位まで取得し、スマートcrop ON時のCPU decode・crop範囲解析は専用workerで次の2表示単位まで準備する。workerは軽量な解析結果だけを返し、Viewer用pixel backingを生成しない。通常GDK Textureの生成・Texture preloadは次の1表示単位に限定する。
- 通常ViewerのByte preloadとSlider thumbnailは単一background schedulerで調停し、preload、明示的なhover-demand thumbnail、preload近傍thumbnail、全域分散thumbnailの順に優先する。低優先thumbnail処理中に高優先要求が来た場合は、現在のAsset境界で譲る。スマートcropのCPU preparationはこのschedulerと分離した専用workerで行い、Byte preloadを長時間blockしない。
- preloadで取得した圧縮画像byteはViewerのByte cacheと近傍thumbnail生成で再利用し、同じAssetをthumbnail目的でarchive / diskから再取得しない。
- スマートcropの切替ではTexture cacheだけを無効化し、crop-result cache、Byte cacheとDocument / logical page位置 / `document_generation` を保持して現在位置を再renderする。Texture再生成時も既知のcrop結果を再利用する。

## スマートcrop

- スマートcropはdefault OFFの永続設定で、通常のViewerページ表示にだけ適用する。Slider hover thumbnail、Library / Search / History / Favorites Cover、archive内容一覧等のthumbnail / Coverには適用しない。
- 判定はdecode済みの物理 `ImageAsset` 全体に対して行い、上端・下端から連続する低情報量余白だけを安全側で除去する。左右方向はcropせず、物理Assetの横幅を維持する。
- crop範囲解析とViewer表示を分離する。専用workerはpreload済みbytesをCPU decodeし、layoutとcrop矩形（またはno-crop）だけの `CropAnalysis` を生成する。解析用のfull-resolution decoded pixelsをprepared結果やViewer texture backingとして保持せず、cropped pixel backingや左右halfのpixel copyも生成しない。
- Viewer表示は通常GDK Textureを基礎とする。crop済みSingleは `PagePaintable` が元Texture全体をcrop矩形に対応するoffsetとscaleで描画し、表示矩形でclipする。intrinsic size / aspect ratioはcrop後の寸法を返す。Pictureは既存の `Contain` / `ScaleDown` で縦横比を維持し、snapshot自体は渡されたwidth / heightへ各軸をmappingする。
- 横長Assetは上下crop後も物理Assetの中央でRight / Leftに分割し、各halfを個別解析しない。両halfは同じsource Textureを共有し、左幅は `width / 2`、右幅は残り（奇数幅では右が1px広い）とする。smart crop OFFでも同じ分割・共有を使う。左右非対称な余白によって見開き中心を移動させず、元のlayout判定、`PagePart`、logical page構造や `spread_shift` を変更しない。
- crop-result cacheは `AssetId` 単位の軽量な判定cacheで、Documentの世代が続く間、Texture / Byteのevictionとは独立に保持する。未解析とcached no-cropを区別し、cached crop / no-cropの表示ではCPUのfull-resolution RGBA decode・crop解析・Texture downloadを再実行しない。cached no-cropのSingleは通常Textureをそのまま表示する。
- `document_generation` / `AssetId` / layoutが一致するready解析結果を一度だけ現在表示またはTexture preloadで消費する。対象置換やDocument切替で不要な準備をcancelし、同一Assetの重複queueを抑止する。先読みを追い越した未解析cache missでは、queue済み / 実行中の重複準備をcancelして同期解析fallbackを使う。idle Texture preloadも同じpreparation handleを使う。
- CPU decoderが拒否しGDKが読める画像では、未解析時だけ元TextureをRGBAへdownloadしてcropを判定する。このfallbackでもViewerは元Textureを保持し、crop結果cache済みの再表示でdownloadやMemoryTexture再構築を行わない。
- JPEGはfull-resolutionの既存smart crop判定経路を維持し、縮小JPEG thumbnailからcropを推定しない。thumbnail disk cache hitはthumbnailだけを供給し、crop-result cacheの根拠にしない。
- 不確実な場合や不自然に小さい結果ではcropせず元画像を表示する。

## ViewerSessionと非同期世代

- `ViewerSession` はDocument、ViewState、ImageCache、`document_generation` を所有し、Document差し替え、ページ移動、現在Texture更新、preload結果の妥当性確認を調停する。
- `load_request_id` は最新のload要求、`document_generation` は採用済みDocumentの世代、thumbnail generationはthumbnail処理の世代であり、意味を統合しない。
- stale load resultは `load_request_id` で排除する。
- preload・遅延decode結果は開始時の `document_generation` と対象 `AssetId` が現在状態に一致する場合だけcacheへ反映する。
- ファイル読込・archive処理などUI threadをblockしうるI/Oは `spawn_blocking` 等で分離し、重い処理をUI threadで直接実行しない。

## archive

- 対応形式はZIP/CBZ、RAR/CBR、7z/CB7、TAR/CBT、LZH/LHA。
- archive形式と対応拡張子は `ArchiveFormat` を唯一の定義元とし、形式固有処理は `archive::formats` に閉じ込める。
- archive形式とaccess strategyは別概念とし、`ArchiveAccessStrategy` の `RandomAccess` / `Sequential` をarchive backend境界で選択する。App / ViewerはRARや7z等の形式名で読込方式を分岐しない。
- Random Accessは前の画像entryを順次decodeせず必要entryを個別取得できる方式とし、`ImageSource::ArchiveEntry` からの実bytes取得をarchive層へ委譲する。ZIPとnon-solid 7zがこの経路を使う。
- 7zはarchive metadataのfolder / stream構造をbackgroundで調べ、各folderにstream entryが最大1つならRandom Access、同一folderに複数streamがあればsolidとしてSequentialを選ぶ。
- non-solid 7zのDocumentは全画像をmemory保持せず、entry indexを持つlazy `ArchiveEntry`を構築する。layout判定は対象entryのprefixだけをdecodeし、通常bytes取得やthumbnailでは対象folderだけをdecodeする。
- Sequentialは前方依存または前方走査を許容して先頭側から順次処理する方式とし、RAR・solid 7z・TAR・LHAがこのstrategyを使う。展開済み画像file bytesの累計が256 MiB以下の間はMemory backingを使い、次の画像で超過する時点に既存・後続画像をすべてDocument所有の一時directoryへ移して `ImageSource::File` とする。この256 MiBはbacking切替thresholdでありhard safety limitではない。一時directoryはXDG user cache配下のAgnam専用temp rootにsession単位で作成し、Document破棄時に削除してarchive再展開や永続cacheには使わない。TAR / LHAはprogress通知非対応のまま一括load完了後に表示するが、保持backingには同じpolicyを使う。
- RAR / solid 7zは画像完成ごとのprogress通知に対応し、solid 7zでは後続entryへ到達するため必要な非画像streamも消費する。progressive表示では最初の表示単位に必要な画像が揃った時点でViewerを開始し、256 MiB超過時は表示中Documentの既存assetも同じtemp-backed storageへ切り替える。全展開が終わるまでViewer開始を遅らせず、完了時のDocumentとの間で全画像bytesを重複保持しない。
- progressive中は未展開画像のlayoutが未確定で最終logical page数を正確に求められないため、Sliderだけはnatural sort後の物理画像indexと総物理画像数を使う。総物理画像数はbackendの既存image planから通知し、Sliderのための追加展開を行わない。全展開完了後は通常のlogical page基準へ戻す。
- progressive中にSliderや通常page移動で未展開画像が要求された場合はtargetをpendingとして保持し、archive workerの順次展開をそのまま継続する。target到達のために別経路の再展開やrandom seekを行わない。待機が200msを超えた場合だけ共通Loading Overlayを表示し、target到着後に移動して解除する。background展開そのものはLoading理由にしない。
- Appがload前に使うprogressive capability判定はI/Oを行わず、実archiveのstrategy判定はbackground load内で行う。non-solid 7zで仮にprogressive stateを準備した場合も、progress通知なしで通常Documentとして完了しstateを破棄する。
- macOS metadata (`__MACOSX`, `._*`) は除外し、entry名はnatural sortする。
- 非nested archiveの自動表紙はmetadata listingでarchive rootまたはtransparent wrapperの `cover.xxx` を選び、対象entryだけを読む。coverがなければ読書対象の自然順先頭entryだけを読み、Sequential形式で前方entryをconsumeする場合も過去bytesの保持や全entryのfilesystem展開は行わない。nested archiveは既存の再帰・一時展開の意味論を維持する。
- 通常archiveはstrategyとbackendに応じてmemory上の画像byteやlazy archive entry等を後続処理へ渡す。すべてのarchiveを常にmemoryだけで処理する前提にはしない。
- nested archiveでは必要に応じて一時directoryへ展開して再帰処理する。再帰上限を維持し、一時ファイルを永続dataとして扱わない。
- 手動展開経路では `..`、絶対path、Windows Prefix等を拒否し、展開先外への書込みを防ぐ。
- Random Access readerは永続global cacheにしない。thumbnail workerでは同一archive readerをworker内で再利用し、ZIP / non-solid 7zともmetadataやsource handleの再openをentryごとに繰り返さない。
- archive resource safetyはoperation-scopedとし、single entryは64 MiB、operation内の実read / 展開byteは累計512 MiB、temporary diskは累計write 512 MiB・live dataの同時occupancy 512 MiB、total entryとimage entryはそれぞれ4,096件まで許可する。累計writeは一時file削除後も維持し、occupancyは一時file / directoryの解放時に減算する。limitちょうどは許可し、超過byte / entryを処理する前に専用resource-limit errorで停止する。これらはuser settingやprocess-global budgetにしない。
- RandomAccess ZIP / non-solid 7zのlazy entry取得は各取得が独立operationであり、Document lifetimeの累積budgetを持たない。Viewer load、Cover取得、archive contents reader/extraction、archive contents thumbnailのarchive readもそれぞれのlifecycleで独立operationとする。既にmaterialize済みのtemp backingを読むだけでは展開量を再計上せず、Bookshelfの永続Cover cacheはtemporary archive disk budgetに含めない。
- nested archiveのViewer loadではouterからdescendantまで同じoperation budgetを共有する。展開画像、nested archive file、descendantへ必要なentry、および一時fileへのwriteを階層ごとにresetしない。
- LZHは `original_size` をsingle-entry limitと照合し、`LhaDecodeReader` を固定bufferでbounded streamingする。RARはcurrent `unrar 0.5.8` APIの制約により、`unpacked_size` metadata preflightとread完了後actual lengthの検査で防御するが、metadataの過小申告時にdecode途中で64 MiBへ強制停止する保証はない。Metadata listing上限は4,096 entries / images。
- resource-limit errorはcancel、破損 / decode、filesystem errorと型で区別する。archive load失敗は現在のLibrary / Viewer上で見えるdialogを表示し、Viewerのlazy entry readではtoastを表示する。progressive loadは既存failure lifecycleを維持し、途中表示をcompleted Documentや正式Historyとして扱わない。cancelはresource-limit error UIを出さない。

## Slider hover thumbnail

- thumbnail生成は物理画像 `ImageAsset` 単位。横長Assetは1回のdecodeからRight / Leftを生成する。
- Viewer Sliderのthumbnail Texture cacheは生成済みpixel backing容量を数える32 MiB soft-budget LRUとする。insertと実際のpreview表示をrecent扱いにし、ready判定のprobeではrecencyを更新しない。raw hover対象と表示中previewは可能な限りevictionせず、それらだけで上限を超える場合は一時的な超過を許容する。
- 32 MiB memory cache missでは、background thumbnail workerが永続disk cacheを調べてからsourceを読む。`glib::user_cache_dir()/agnam/slider-thumbnails/<Document group hash>/<source fingerprint + Asset identity hash>.rgb` にversion付きRGB pixelを保存し、SpreadのRight / Leftは1つのatomic fileとして扱う。disk hitでも既存のgeneration検証を通し、corruption・欠損・write失敗は通常生成へのmissとする。
- fingerprintは直接画像のstable path、または外側archiveのstable pathにsize・mtimeを組み合わせ、Unixではdevice・inode・change timeも使う。Random Access entryはindexとentry名、Sequentialは自然順physical index、nestedはlogical archive chain・image path・physical indexを使う。Sequential spillやnested展開のtemporary physical path、session固有の`AssetId`は永続identityに含めない。
- disk cacheは生成済みAssetだけをbest-effortで保存し、cacheのための全page先行生成は行わない。1 GiB soft limitを超えた場合はbackgroundで最近使っていないDocument groupから削除し、Document open時にlast-usedを更新する。cache directoryの削除やcleanup失敗は閲覧を妨げない。
- 通常Documentの全域background生成順はarchive全域の最大空白区間を段階的に分割するanchor順を使い、各anchorを中心・前隣・後隣の最大3ページblockとして扱う。6ページ単位のbackground burstを維持する。
- 通常Documentではpreload対象近傍のthumbnailを全域分散thumbnailより優先する。preload済み / cache済みbytesから生成し、成功したAssetは全域分散側でskipする。近傍生成に失敗したAssetは全域分散側で再試行可能とする。
- 通常Documentでraw hover対象がthumbnail cache missの場合は、そのAssetを同じbackground schedulerへ最新需要としてオンデマンド要求する。未開始の古いhover需要は置き換え、workerの分散生成済み履歴に含まれるAssetでもLRU eviction後は再生成可能とする。全域分散schedule自体の順序はSingle / Spreadやhover位置では並べ替えない。
- 全域分散生成はDocument全件生成ではなくcache warmupを目的とし、32 MiB soft budgetへ到達したgenerationでは停止する。preload、hover-demand、preload近傍thumbnailはその後も動作する。
- サムネイル生成速度はLow / Normal / Highの3段階で永続化し、デフォルトはNormal。速度制御は全域分散thumbnailだけに適用し、Highは人為的な待機なし、Normalは概ねwork 2 : rest 1、Lowは概ねwork 1 : rest 2を維持しつつ各Asset処理後に最低100ms休止する静音・低負荷優先とする。待機中でもpreload / 近傍thumbnail、Document切替、thumbnail cancel、速度変更へ即応できるようにする。
- preload、通常 / progressive中のhoverオンデマンドthumbnail、近傍thumbnail、archive展開自体には生成速度によるpacingを適用しない。
- raw hover位置と実際に表示するpreview対象を別状態として管理する。raw位置が変わった時にready集合から最寄りを選び、同距離なら小さいindexを優先する。
- hover開始時にready previewを表示できた場合は、より近いthumbnailが後からreadyになっても、そのhover中の表示対象を維持する。ready previewがなくplaceholderだった場合だけ、raw targetのオンデマンド生成完了時にpointerを動かさずpreviewへ昇格してよい。
- Spread previewは構成する全pageがreadyの場合だけ表示対象とする。
- RAR / solid 7zのprogressive表示中は通常のbackground ThumbnailWorkerを開始せず、hoverされた物理画像に必要なAssetがすでに展開済みの場合だけ、保持済みMemory bytesまたはtemp fileからthumbnailをbackgroundでオンデマンド生成する。thumbnail目的でarchiveを再open・再展開したり、未展開targetへ先回りしたりしない。
- progressive中の最寄りready判定は物理画像indexの距離を使い、logical page indexと混在させない。表示単位への対応付けはViewerの現在のSingle / Spreadと `spread_shift` を反映する。
- progressiveオンデマンド生成は同時に1 jobを基本とし、hoverが移動した場合は未開始queueを最新需要へ置き換える。同一Assetの重複生成を抑え、stale session / thumbnail generationの結果はcacheへ反映しない。
- 全展開完了後は通常のbackground ThumbnailWorkerへ切り替え、progressive中に正常に生成され現在もcacheに残るAssetはskipする。生成履歴だけでLRU eviction済みAssetを永久にskipせず、通常hover-demandから再生成可能とする。オンデマンド生成に失敗したAssetや完了境界でin-flightだったAssetも通常workerで再生成可能とし、重複回避よりcorrectnessを優先する。

### JPEG

- JPEG thumbnailはTurboJPEG/libjpeg-turboのdecompression scalingを利用する。
- 原則として目標200px以上を満たす最小scaleを使うが、1/8結果の高さが175px以上なら200px未満でも1/8を採用する。200px超ならTriangle filterで高さ200pxへ最終縮小する。
- decode後からGTK `R8g8b8` MemoryTextureまでRGB 3bytes/pixelを維持し、strideを明示する。
- TurboJPEG Decompressorはworker内で生成・再利用しUI threadへ渡さない。
- 現在のbuildでは `pkg-config` 経由の `libturbojpeg` とSIMD supportを前提とする。

### PNG / WebP

- `image` crateで `DynamicImage` へdecodeし、高さ200px超の場合だけRGB8変換前に `thumbnail_exact()` で200pxへ縮小する。
- 小画像は拡大しない。最終結果はRGB 3bytes/pixel、stride=`width * 3`。
- 通常Documentのthumbnail生成とsmart crop解析が同時期にdecode済み `DynamicImage` を所有している場合だけ、同じ `document_generation` / `AssetId` のimageを `Arc` で共有して重複decodeを省く。共有registryは `Weak<DynamicImage>` のみを保持し、最後の処理所有者が解放したfull-resolution pixelsを長寿命cacheにしない。Document世代切替でregistryをclearし、stale世代から現在のpixelsを再利用・登録しない。
- decode / analysis / thumbnail縮小は共有lockの外で行う。decode完了待ちや高並列decodeを追加せず、両workerが同時に共有missした場合は各自decodeしてよい。Byte preload / thumbnail schedulerとsmart crop解析workerの責務・優先順位は維持する。JPEGとthumbnail disk cache hitはこのdecode共有を通らない。

### thumbnail ON/OFF

- サムネイルOFFではDocument読込時にworkerを開始せずhover表示もしない。progressive中のオンデマンド生成も開始しない。
- 実行中にOFFへ変更した場合はgeneration/cancelで旧workerを無効化し、progressiveのpending需要、Popover・hover状態・cacheを消去する。
- ONへ戻した時点で通常Documentならbackground生成を開始し、progressive表示中なら以後のhoverからオンデマンド生成を利用する。
- 生成速度の設定値はOFF中も保持し、ONへ戻した通常Documentの全域分散生成へ適用する。
- generation、生成途中状態、Popover状態、thumbnail cacheは永続化しない。
- Thumbnail WorkerはApp/MsgやUI層のsenderに依存せずcallbackへ結果を返し、Appが既存Msgへ橋渡しする。

## 前後ファイル移動とload

- 前後表示対象は現在画像/archiveを基準にnatural sortで探索する。UI threadからはbackground実行する。
- 明示的な前後Document移動では `↑` / `[` で前のDocumentへ戻り、`↓` / `]` で次のDocumentへ進む。独立した世代付きsibling探索を先に行い、探索中は現在Documentとprogressive workerを維持する。siblingなしはloadやcancelを開始しないno-opとし、staleな探索結果は無視する。
- siblingが見つかった場合も次Documentが表示可能になるまで現在Viewerを保持し、ViewerからViewerへ直接切り替える。旧progressive workerはreplacement開始時にcancelしてよいが、そのcancelではprogressive開始前のLibrary等へrollbackせず、表示中の部分Documentとtemp storageを次Documentへの切替まで保持する。replacement loadの失敗時もLibraryへ遷移しない。
- 通常ページ送りでDocument端を越える場合だけ、設定に従って前後Document間の境界ページを経由する。明示的な前後ファイル移動は直接loadする。
- 境界ページはAppの一時表示状態であり、実 `Document` / `Page` や `ViewerSession` のpage modelへ追加しない。page index、ページ数、Slider、thumbnail、履歴、last session、読書進捗にも含めない。
- 境界表示用の隣接Pathだけをbackgroundで探索し、境界表示時点では隣接Documentのarchive展開、画像decode、Texture生成を開始しない。実際に隣接Documentへ進む操作で通常のload経路を開始する。
- 境界ページから隣接Documentをloadする場合は、新Documentのloadが完了するまで境界表示を維持し、保持中の旧Document画像を途中で再表示しない。対応するloadの完了・失敗時にだけ境界状態を解除し、staleなload結果で現在状態を壊さない。
- 新しいDocumentが正常にViewerへ表示された場合は、入口にかかわらずDocument表示名を3秒のToastで通知する。境界ページ自体、同一Document内のページ移動、load失敗・stale結果では表示せず、連続移動時は最新のToastだけを残す。Toast表示中にViewerを離れて本棚へ戻る場合は、その時点でToastを終了する。
- load要求開始時はLoading layerを隠した状態で200ms後の表示通知を予約し、その時点でも同じrequestがactiveの場合だけ表示する。
- load成功では最初のpage Texture反映後に完了、失敗や前後対象なしでは完了通知時にactive stateを終了する。
- 古いrequestのtimer・成功・失敗・探索結果は新requestへ影響させない。

# UI設計仕様

この文書はUI全体の安定した仕様・設計判断をまとめる。UI、設定、HeaderBar、Slider、fullscreen、Loading表示を変更する作業で必要な箇所だけ参照する。

## トップレベル構成

- トップレベルUIは左端の常設navigation rail、その右の `AdwOverlaySplitView`、さらにそのcontent内の共有 `AdwToolbarView` / Library / Viewerで構成する。
- navigation railは幅48pxでLibrary / Viewerの双方に表示し、fullscreenでも維持する。背景はLibadwaitaのsidebar用CSS変数に追従し、常設navigation領域としてメインcontentと視覚的に分離する。
- rail上部は `go-up-symbolic` の「上の階層へ」、4pxの追加marginによる小さな余白、検索、履歴、お気に入りの順とする。各主要buttonの操作領域は40×40pxを基本とする。上位移動はbreadcrumbの現在階層から親を求め、履歴のBackとしては動作しない。親階層がない本棚rootではbuttonをdisabledにする。
- メインメニューは `open-menu-symbolic` のMenuButtonとしてrail下端に置き、navigation項目から分離する。
- breadcrumbは本棚rootのhome iconを先頭segmentとして、下位階層・archive・Viewerの現在位置まで表す。home segmentのtooltipは「本棚トップ」とし、root自身以外ではクリックによりrootへ移動する。本棚root自身ではhome segmentを現在位置として表示し、自己navigationを行わない。HeaderBar中央の「Agnam」は表示しない。
- お気に入り入口は `starred-symbolic` のToggleButtonとし、選択中かどうかは他panelと同様にbuttonのactive背景で示す。選択状態によってstar icon自体は切り替えない。
- 検索・履歴・お気に入りは同じ左drawerを共有する。drawerは `AdwOverlaySplitView` のcollapsed sidebarとしてメインcontentを押し縮めず上へoverlayし、標準scrimを表示する。幅は280～320pxを基本とし、sidebar width fractionは0.3とする。
- 同じrail buttonを再度押すとdrawerを閉じ、別buttonを押すとdrawerを閉じずに内容だけ切り替える。active状態は現在表示中のpanelだけに付ける。drawer外のメインcontentをクリックした場合も閉じる。
- drawerのcloseは表示を隠すだけとし、検索・履歴・お気に入りの保持状態やWidget treeを破棄しない。Search / History / Favorites切替でも各panelの状態を保持する。
- `gtk-enable-animations=false` 環境では `AdwOverlaySplitView` の2回目以降のopen時にscrim allocationが反映されない場合があるため、`show-sidebar=true` 通知後の次のmain-loop idleで `queue_allocate()` を1回要求する。`queue_draw()` だけではこの問題を解消できないため、この補正を不用意に削除しない。
- Loading Overlayはrail / drawer / HeaderBarを覆わず、メインcontentだけを覆う。
- archive読込中のresource-limit failureは、Library / Viewerのどちらから開いてもメインwindow上のdialogで通知する。History / Favorite起点では各専用の失敗dialogを使う。archive内容の逐次thumbnail生成が同じ上限で停止した場合は、現在のLibrary sessionに限って1回通知する。

## HeaderBar

- `UserSettings.header_auto_hide` は永続設定、pointer位置と現在のreveal状態は `HeaderController` のランタイム状態として分離する。
- Libraryでは通常時・fullscreen中ともHeaderBarを常時表示し、通常のHeaderBar分の領域を確保する。
- Viewerでは通常時は設定ONの場合だけ自動非表示、fullscreen中は設定値にかかわらず自動非表示にする。
- 自動非表示中は画像領域上へのoverlayとし、テーマのHeaderBar背景色を80%の不透明度で適用し、画像との境界に薄い影を表示する。
- ウィンドウ上端から50px以内で表示、表示中に100px以上離れると非表示、50px～100pxでは現在状態を維持する。
- HeaderBar上にpointerがある間は表示を維持する。ウィンドウ外へpointerが出た場合は非表示へ戻す。
- navigation drawerやrail上のメインメニューのopen状態はHeaderBarの表示条件に含めず、それらを開いていることだけを理由にHeaderBarを表示・固定しない。
- ViewerからLibraryへ戻った直後は常時表示・通常レイアウトへ戻す。
- HeaderBar中央の「Agnam」は表示せず、本棚root画面を含めてbreadcrumbを使用する。

## HeaderBar上の操作

Viewerでは「1ページ進む / 見開き / 1ページ戻す」のLinked Buttonグループを表示する。

- 左右の単ページ移動buttonはSpread表示中だけ操作可能。Single表示中も非表示にはせず、Linked Buttonの形状を維持する。
- 「見開き」は常に `view-dual-symbolic`、ツールチップは「見開き」。ON/OFFはToggleButtonのactive状態で表し、アイコンは切り替えない。
- 「小さい画像を拡大する」は設定画面から変更し、HeaderBarには表示しない。
- 設定画面の「ページ表示」には「スマートcrop」Switchを置く。defaultはOFFで、Viewer表示時だけ外周余白の自動除去を有効にする。
- Viewer HeaderBarには `image-crop-symbolic` の「スマートcrop」ToggleButtonを表示する。tooltipは「スマートcrop」、active状態は `UserSettings.smart_crop` を表す。ページ移動のLinked Buttonグループとは独立して配置し、設定画面のSwitchと同じ永続設定を操作する。Libraryでは表示しない。
- LibraryではViewer専用buttonを表示しない。
- Libraryでは `view-list-ordered-symbolic` の並び替えMenuButtonを表示し、ツールチップは「並び替え」とする。検索はdrawer内で行うため、検索中も通常Libraryの並び替えbuttonを隠さない。
- 並び替えPopoverは「ファイル名 / 更新日時 / 作成日時」と「昇順 / 降順」の2セクションで構成し、stateful Actionの標準radio/check表示で現在値を示す。button icon自体は並び替え方向で切り替えない。
- メインメニューと検索・履歴・お気に入りの入口はHeaderBarではなく左navigation railへ集約する。

## メインメニューとアクセラレータ

navigation rail下端のメインメニューは次の構成を維持する。

1. 「ファイルを開く…」 (`<Primary>o`)
2. separator
3. 「フルスクリーン」 (`F11`)
4. 「キーボードショートカット」 (`<Primary>question`)
5. separator
6. 「設定」 (`<Primary>comma`)
7. 「Agnam について」

- メインメニューは `open-menu-symbolic` のMenuButtonとしてrail下端に常設し、HeaderBarには重複配置しない。
- 本棚検索はメインメニューへ置かず、左railの検索buttonまたは `<Primary>f` から開く。
- 検索ActionはLibrary / Viewerの双方で、本棚root設定済みの場合に有効とする。Viewerから開いてもViewerをLibraryへ切り替えない。
- ViewerでSearch panelを表示している間はページ移動・ファイル移動のViewer acceleratorを無効にし、SearchEntryのBackspace / Left / Right / Space等の通常編集を優先する。Search panelを閉じるとViewer acceleratorを再度有効にする。History / Favorites panelではViewer acceleratorを無効にしない。
- ページ移動・ファイル移動の閲覧操作はメインメニューへ直接並べない。
- About画面にはアプリケーション名、開発者名、Cargo package versionを表示する。

## ページ移動Slider

- `UserSettings.slider_auto_hide` は永続設定、現在の表示状態やdrag状態はSlider側のランタイム状態として分離する。
- 自動非表示ONでは画像領域のサイズを変えないoverlay、OFFでは従来どおりSlider高さ分だけ画像領域を狭めて常時表示する。デフォルトはON。
- fullscreen中は設定値にかかわらずoverlayの自動非表示とし、解除後に現在の設定値へ戻す。
- 自動非表示ではウィンドウ下端から50px以内で表示、100px以上離れると非表示、中間では現在状態を維持する。
- Slider上にpointerがある間とdrag中は表示を維持する。
- RAR / solid 7zのprogressive表示中も通常のSlider配置と自動非表示規則をそのまま使い、background展開専用の下部statusへ置き換えない。展開中であることだけを理由に画像領域の高さを変えない。
- progressive中は未展開画像のlayoutが未確定なため、Sliderのrange・value・page labelはnatural sort後の物理画像index / 総物理画像数を使う。全展開完了後は通常のlogical page基準へ戻す。横長AssetのRight / Leftはprogressive中だけ同じ物理画像位置を共有する。
- progressive Sliderで既展開の物理画像を指定した場合は即時移動し、未展開targetの場合は順次展開の到着を待つ。target待機中もSlider領域を消さず、pending位置をSlider上で維持する。
- ページプレビュー設定では表示位置に加えて「サムネイル生成速度」を低速 / 標準 / 高速から選択できる。サムネイルOFF時は表示位置と生成速度の設定Widgetを操作不可にするが、選択値自体は維持する。

## Document境界ページ

- 通常フォルダ内では、通常ページ送りで現在Documentの端を越える際に前Documentと次Documentの関係を明示する境界ページを挟む。同じDocument間では進入方向にかかわらず同じ表示にする。
- 本棚root直下のBookは互いに独立したタイトルとして扱い、通常ページ送りでも明示的な「次ファイル / 前ファイル」操作でも別Bookへ移動しない。Document端で停止し、境界ページも表示しない。
- 境界ページは設定でON/OFFでき、デフォルトはON。OFFでは通常フォルダ内の隣接Documentへ直接移動する。
- 境界ページではSlider、ページ番号、hover thumbnailを表示せず、Slider常時表示設定でも予約領域を残さない。
- 通常フォルダ内の明示的な「次ファイル / 前ファイル」操作では設定にかかわらず境界ページを挟まない。
- 境界ページの具体的な視覚designは今後調整可能とし、前後Documentの関係が明確であることと、操作可能なbutton等に誤認されにくいことを優先する。

## fullscreen

- fullscreenは永続設定ではなく `GtkWindow:fullscreened` 通知から更新するランタイム状態。
- fullscreen要求時点ではなく、実際のfullscreen状態を基準にHeaderBar / Slider / title buttonの実効状態を決める。
- fullscreen中はLibraryのHeaderBarを常時表示、ViewerのHeaderBarを強制auto-hide、Sliderを強制auto-hideにする。
- fullscreen中もnavigation railは表示し、drawerのopen/close状態は強制変更しない。標準window title buttonはstart/endとも非表示にする。
- fullscreen解除後は現在画面と設定値から実効状態を再評価し、開始前のvisible状態そのものは復元しない。
- fullscreen由来のサイズ・最大化状態は通常ウィンドウ状態として保存しない。

## Loading表示

- 共通load requestで画像・通常archive・nested archiveを読み込む際、要求開始から最初のページTexture反映まで200msを超えた場合だけLoading Overlayを表示する。
- RAR / solid 7zのprogressive表示開始後は、background展開が続いていること自体ではLoadingを表示しない。現在のpage移動またはSlider移動で未展開targetを要求し、その待機が200msを超えた場合だけ同じ共通Loading Overlayを表示する。
- progressive targetが200ms以内に利用可能になった場合はOverlayを出さず、到着したら要求位置へ移動する。待機中もSliderの表示・配置は維持する。
- 表示内容は半透明の暗いレイヤー、GTK Spinner、「読み込み中…」。進捗率や専用設定は持たない。progressive用の別Loading表示や下部の「展開中…」等は設けない。
- 新しいDocumentの読み込み成功までは既存画像・画面を残す。
- Overlay表示中は下のメインcontentへの入力を遮断するがHeaderBar、navigation rail、drawerは別階層として維持する。
- Loading表示のactive request、表示状態、遅延通知revisionは `DocumentLoadController` が一体で所有し、画面種別やprogressive専用に重複したLoading stateを持たない。

## 設定の所有と永続化

- 永続設定の唯一の信頼できる状態はAppが所有する `settings::UserSettings`。設定画面、HeaderBar、Input、Action、PreviewController、SliderControllerへ同じ設定値の独立所有状態を作らない。
- navigation panelのopen状態や選択pageはruntime stateであり設定へ保存しない。
- 設定画面Widgetは `UserSettings` の投影として扱い、Appから同期する際はsignalをblockし、変更はAppの共通更新経路へ戻す。
- 設定はGLib KeyFileでユーザー設定ディレクトリ配下の `agnam/settings.ini` へ保存する。ファイル欠損・個別キー欠損・不正値では該当するデフォルトへfallbackし、起動を妨げない。
- 現在の主要デフォルトは、ページ移動=マウスボタン、Document境界ページ=ON、小さい画像の拡大=ON、スマートcrop=OFF、Spread=ON、HeaderBar自動非表示=OFF、Slider自動非表示=ON、サムネイル=ON、サムネイル位置=マウス追従、サムネイル生成速度=標準、本棚root=未設定、フォルダpreview高さ=78px、direct file Cover高さ=156px、本棚並び替え=ファイル名・昇順、起動時動作=本棚トップ、last sessionなし。
- 廃止済みキーは読み飛ばし、次回保存時には現在の設定だけを書き出す。旧 `slider-bar-mode` は既存migration規則を維持する。

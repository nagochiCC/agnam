# 履歴設計仕様

この文書は閲覧履歴のidentity、永続化、更新タイミング、履歴drawer、Cover取得に関する安定した仕様・設計判断をまとめる。関連作業で必要な箇所だけ参照する。

## 履歴の単位とidentity

- 履歴は単純なDocument列ではなく作品単位で保持する。
- 本棚root配下では基本的にDocumentの親directoryを作品identityとする。
- filesystem画像群では、画像Documentの親directoryを画像Bookとして扱う。画像Bookがroot直下でなければ、親directory配下に直下画像を持つ別の画像Bookが存在する場合は巻名に依存せず、その親directoryを作品identityとして共有する。
- 画像Bookのdirectory名が次の保守的な巻名パターンに一致する場合も、root境界を越えない範囲でさらに1階層上を作品identityとする。
  - 数字のみ
  - 数字 + 「巻」
  - 「第」+ 数字 + 「巻」
  - ASCII大小文字を区別しない `vol` / `volume` + 省略可能な `.` または空白 + 数字
- root直下Document、root直下にある画像Book、本棚root外Documentでは安全な従来範囲へfallbackし、本棚root全体へ統合しない。
- archiveの作品判定は親directory単位を維持する。
- 同一作品の別巻・別画像Bookを開いた場合は同じ履歴entryの最後のDocument、logical page、総page数、作品読了状態（`at_document_end`）、最終閲覧時刻を更新し先頭へ移動する。

## 記録・更新

- 正常かつ表示pageを持つDocumentが共通load完了経路で採用された時点でidentityを確定し、Appが現在Document用identityを保持する。
- stale結果、load失敗、page 0件は履歴へ記録しない。
- 閲覧中に本棚root設定が変わっても、すでに開いているDocumentのidentityは再計算しない。
- Viewer状態変更後の共通refresh経路でlogical page、総page数、現在の表示単位が現在Document末尾まで到達しているかを確認し、いずれかが変化した場合だけ履歴を更新・保存する。
- `ViewerSession::at_document_end()` は現在Documentの末尾到達だけを表す。`HistoryIdentity::BookshelfWork` では、現在Document末尾に到達し、かつ同じ作品identity配下に次のDocumentが存在しない場合だけ履歴の `at_document_end` を `true` にする。次のDocumentが別作品identity配下ならその作品の読了判定には含めない。`BookshelfWork` ではない独立Documentは現在Document末尾への到達をそのまま読了状態とする。
- 現在Documentの末尾到達は保存page番号の単純比較ではなく、Single / Spread、`spread_shift`、横長画像由来のlogical pageを含むViewerの表示単位で判定する。末尾表示から前へ戻れば末尾到達状態も解除し、履歴の読了状態も解除される。
- `at_document_end == true` のentryでも最後に閲覧したlogical pageは履歴情報として保持するが、そのpageを途中再開位置としては使用しない。再openはDocumentの通常先頭位置から開始し、読み込み成功後は通常の履歴記録へ合流して読了状態を解除する。末尾から前へ戻って `at_document_end == false` になった場合は、その保存pageを再開位置として再び使用する。
- 履歴entryを閲覧中に削除した後、同じDocument上の `update_position()` だけでentryを復活させない。新しくDocumentを開き直した場合だけ通常の記録処理で再登録する。

## 永続化

- 履歴は `UserSettings` / `settings.ini` と分離し、Appが `HistoryStore` と現在Document用 `HistoryIdentity` を所有する。
- 保存先はユーザー設定directory配下 `agnam/history.ini`。
- GLib KeyFileを使い、entry順を新しい順として保存する。
- 現在のformat versionは2。version 1以前はmigrationせず空履歴として扱い、次の記録時にversion 2で保存し直す。`settings.ini` には影響しない。
- format version 2では各entryに読了状態 `at-document-end` を保存する。同keyが存在しない既存version 2履歴は `false` として読み込み、曖昧な保存pageから読了を推測しない。
- 現行の本棚rootがある場合、読み込み時に本棚内の画像Document entryを現在の作品identity規則で再正規化する。旧実装で同一作品の兄弟画像Bookが別entryとして残っている場合は、新しい順のentryを残して重複を除去し、正規化結果を保存し直す。
- Path存在確認はload条件にしない。履歴file全体を読めない場合は空履歴、個別entryの必須値が壊れている場合はそのentryだけ無視する。
- 保存失敗はstderrへ報告するだけでViewer動作自体を失敗させない。
- UIはStore内部Vecを直接変更せず、公開された参照・identity削除・全削除APIを使う。

## 履歴drawer

- Library / Viewer共通の左navigation railにある履歴buttonから、検索と同じ `AdwOverlaySplitView` sidebarへ履歴を表示する。
- 同じ履歴buttonを再度押すとdrawerを閉じ、Searchへ切り替えた場合はdrawerを開いたまま内容だけ切り替える。
- drawer closeやSearchへの切替ではvirtualized list、現在表示内容、scroll位置、realized rowの取得済みCover stateを破棄せず、一時停止として扱う。
- 履歴が閉じている間に `HistoryStore` が更新された場合はpanelをdirtyとして記録し、次回open時に最新entriesを再投影する。変更がなければ既存Widgetを再利用する。
- drawer open状態はHeaderBarの表示条件に含めず、Viewer HeaderBar自動非表示の実効条件を変更しない。

## row表示と操作

- entryは新しい順にdrawer内へ縦表示し、件数が多い場合は縦scrollする。rowはrecycling可能なvirtualized listで管理し、画面外を含む全entry分のButton、Picture、Popover等を常時生成・保持しない。
- rowは小型Coverと作品名を表示し、作品内の最後のDocumentが作品名と異なる場合はそのDocument名を専用の1行へ表示する。archive / fileは拡張子を含むfile名、filesystem画像Bookは画像file名ではなく画像Book folder名を使う。Document名が不要なroot直下の独立Documentや作品folder自体が画像Bookのケースでは空行を残さない。最下段は `現在page / 総page数 · 最終閲覧日時` とし、長い作品名・Document名はdrawer幅を押し広げず1行でellipsizeする。
- rowの通常クリックは続きを読む操作とし、drawerを閉じて保存Document pathを既存共通load経路へ渡す。`at_document_end == false` なら保存logical pageから再開し、`true` なら保存pageを使わずDocumentの通常先頭位置から開始する。pageは現在のViewer表示モードのsnap規則に従う。
- rowを右クリックするとクリック位置にcontext Popoverを表示し、「履歴から削除」から対象entryだけを確認dialogなしで削除できる。右クリックでは続きを読む処理を発火させない。
- 履歴自体はPopoverではなくnavigation drawerなので、個別削除用context Popoverは以前のnested Popover制約を受けない。row再描画やView破棄時は手動parentしたPopoverを適切にunparentする。
- 個別削除後は表示中の履歴drawerをその場で最新内容へ更新する。
- 履歴・お気に入りのvirtualized listでは、entry削除時にGtkListView側の挙動でscroll位置が先頭へ戻る場合がある。第二段階では既知制約として許容し、複雑なscroll復元処理は導入しない。
- drawer HeaderBarの全削除buttonはdestructive確認dialogを表示し、確認後に全削除する。
- 0件ではdrawer内部にempty stateを表示する。
- 履歴Documentを開けなかった場合は対象Pathを示すdialogを表示し、「閉じる」または「履歴から削除」を選べる。Path不在だけで自動削除しない。

## Cover取得

- 履歴Coverは本棚の検証済みdisk cacheとCover生成処理を再利用する。
- ただしscheduler、generation、pending、active、Texture cache等の表示scopeは通常本棚・検索から分離する。
- cache lookupは最大8件、generationは最大2件を基本とする。
- Cover需要はrealized / visible付近のrowだけを対象とし、scrollで需要が変わった場合は未開始の古いpendingを破棄する。rowのunbind時はpaintableとtarget参照を解除し、非同期結果はgenerationに加えて現在bind中のsourceが一致する場合だけ反映する。
- drawer close / Search切替では新しいjobのscheduleをpauseするが、targetと取得済み状態は保持する。再open時に内容がdirtyでなければresumeして残りjobを続行する。
- 履歴内容が更新され再描画する場合だけgenerationを更新し、古いbackground結果を現在Widgetへ反映しない。

# Agnam repository instructions

このfileにはAgnam固有のarchitecture・security boundary・実装 / 検証制約だけを置く。共通の開発原則、Source of truth、Git運用、Security / Privacy、一般的な検証ルールはglobal `~/.codex/AGENTS.md` に従う。

## Agnam固有の実装制約

- folder / archive / 各形式で挙動を統一するかどうか自体が仕様判断になる場合、current source of truthから確定できなければ勝手に統一しない。
- 明示的な仕様変更なしに、既存UI、操作体系、accelerator、設定互換性、状態所有、非同期処理、archive安全性を変更しない。
- 永続設定はAppが所有する `settings::UserSettings` を信頼できる状態とし、設定画面やcontrollerへ同じ設定値の独立所有stateを作らない。
- `ViewerSession` が現在の `Document`、`ViewState`、Viewer cache、`document_generation` を所有する。App側へDocument内部dataを重複所有しない。
- UI signal / Action / background結果は既存の `AppSender` → `Msg` queue → `App::dispatch()` → `sync_ui_state()` のevent経路を維持する。`load_request_id`、`document_generation`、thumbnail generation等の別目的のgenerationを安易に統合しない。
- archive形式固有処理は `archive::formats` へ閉じ込め、formatとaccess strategyを分離した既存境界を維持する。nested archiveの再帰上限、展開先directory外への書込み防止、一時fileを永続identityに使わない等の安全性を弱めない。
- GTKの複雑なruntime state / event経路やarchive処理は、局所的な見た目だけでなくstate所有・stale結果・cancel・error経路まで含めて変更影響を確認する。

詳細なcurrent仕様はこのfileへ複製せず、`docs/README.md` から該当するcurrent source of truthを参照する。

## Agnam固有の検証

- Rust codeを変更した場合は内容に応じて `cargo fmt --check`、`cargo check`、必要な関連testを実施する。
- GTKの見た目、操作感、runtime挙動などGUIでしか確認できない事項は、必要に応じてユーザー確認を完了条件とする。
- 明示要求がない限りAgnamを起動してGUI検証を行わない。必要なGUI確認が残る場合は完了報告で明示する。

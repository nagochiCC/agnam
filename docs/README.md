# Documentation index

この文書はAgnamのcurrent source of truthへの固定入口である。repositoryに関する仕様・設計・current stateを確認するときは、ここから必要なdocumentだけを辿る。

## Current source of truth

- [`STATUS.md`](STATUS.md): current implementation / verification stateの短いsnapshot
- [`design/ui.md`](design/ui.md): UI全体、navigation、HeaderBar、Slider、fullscreen、設定、Loading、Document境界page
- [`design/library.md`](design/library.md): Library、本棚、Cover、検索、お気に入り、起動復元、読書進捗
- [`design/viewer.md`](design/viewer.md): Viewer、Document / Page、Spread、cache、archive、thumbnail、非同期load
- [`design/history.md`](design/history.md): 履歴identity、永続化、drawer、削除・続きを読む、Cover取得
- [`design/archive-progressive-loading.md`](design/archive-progressive-loading.md): RAR / 7zの段階的読み込み

詳細仕様は各documentをsource of truthとして維持し、このindexや`STATUS.md`へ全文複製しない。

## Other repository documents

- [`../README.md`](../README.md): 利用者向けの製品概要、対応形式、build / run、基本的な使い方
- [`../AGENTS.md`](../AGENTS.md): Agnam固有の実装・検証制約

## 読み方と更新

最初から全documentを読む必要はない。まずこのindexから、必要に応じて`STATUS.md`と現在の相談・設計・実装・reviewに関係するdocumentだけを読む。

source of truthの追加、改名、責務変更があった場合はこのindexを同期する。

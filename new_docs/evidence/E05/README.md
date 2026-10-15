# E05：セルと流体 — 実装中

## 判定

**E05全体は未完了。Q04は未通過。** この記録を工程の完成証拠として使わない。

作業branchは`codex/e05-cell-and-fluid`、開始点はE04マージ後の`3ae60e20a34f7162f739988d25d332045bc52967`。E05以降の作業順序は[工程表](../../19_IMPLEMENTATION_ROADMAP.md)に従う。

## 実装した範囲

- layerへ所属する物質channelの登録とContentへの所有権記録。
- cell本体と全付随channelの原子的交換・経路輸送。
- 活動域、密度交換、一phaseの再移動防止を扱う局所輸送の参照処理。
- 自由表面、閉領域anchor、重力項を扱う圧力のJacobi緩和。
- 固定件数で進め、World状態として保存・再開できる押出し経路探索と一括公開。

APIの契約・容量費用・未対応範囲は[30_MATERIAL_TRANSPORT](../../30_MATERIAL_TRANSPORT.md)を参照。

## 検証

対象は`tests/material_transport.rs`。輸送中の一channelの容量不足と無視されたエラーの全体rollback、chunk境界での確保回収、負座標と原点変換、blockの拒否、活動域、物質数保存、保存再開の結果一致、圧力式の参照値、検索frontier保存、上向き水路、二重公開、入力失効、record切断、区域退避・復帰、channel所有権と格子の登録検査、channel書込を含む編集予算とWorld全体のchannel byte上限を対象とする。

### ローカル検査

2026-10-16、Windows/MSVC。依存cacheは`target/e01-validation`、TEMP/TMPはAドライブ上の`target/e05-build-tmp`へ指定。Cドライブの空き容量不足に依存してlinkerが失敗しないよう、process単位で作業領域を分けた。

- `cargo fmt --all -- --check`：成功。
- `cargo check --all-targets --locked`：成功。
- `cargo clippy --all-targets --locked -- -D warnings`：成功。
- `RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps`：成功。
- 全243テスト：最後のnode容量変更API追加後の最終ツリーで再実行して成功。内訳はlib/host 103、assets 24、foundation 19、material_transport 19、runtime 28、world 50。

表記した各cargoコマンドには共通して`--target-dir A:/Project/charon-engine/target/e01-validation -j 1`を指定した。fmtには指定しない。GitHubのE05専用workflowでWindows・Ubuntu・macOSの対象試験を実行するが、ローカル検査をその3 OSの成功として扱わない。

## ローカルレビューでの修正

- 輸送先を順に確保すると、旧chunkが空になる場合にも不要な容量超過となる。旧値退避→解放→再配置へ修正。
- 保存recordの形式検査だけでは地形との整合が不足する。探索の再開・公開時に物質、空き先、物性と圧力条件を実地形と照合する。
- 予約のsorted Vec挿入はcell数に対し二乗の費用になる。活動chunkごとの4096bitへ変更し、chunk検索とbit演算で処理する。
- 地形だけの編集予約では、付随channelの書込数を予算から漏らす。所属channel数を含めて先に全経路を予約し、編集容量とchannel byte容量の両方でrollbackするテストを追加。
- 探索容量を使い切った後もfrontierを維持して再試行できるよう、node上限の明示変更APIを追加。

## 残作業

[30の残作業](../../30_MATERIAL_TRANSPORT.md#e05の残作業)を実装・検証する。今回の変更はE05内の「輸送と圧力」のレビュー単位であり、E05全体の完了を示さない。この単位をcommit/push・PR・外部レビュー待機・指摘対応まで進め、続いてE05内の残りの実装を行う。E05の出口条件を小さく定義し直さず、未達を残してE06へ進まない。

# E05：Q16物性と速度輸送への標準・独自provider統合

## 保存契約と不変条件

`MaterialBuilder::motion_properties` は `MaterialMotionProperties` の重力倍率、抗力、浮力、摩擦を素材Contentへ登録する。各係数は無次元Q16。抗力・摩擦は `0..=65536`、重力倍率・浮力倍率は非負の`u32`とし、最終Q16速度へ変換できない計算を成功として扱わない。物性を明示した素材はContentのmanifestへ拡張領域 `MOTNPX01` を付記し、値だけ変更した別Contentとの保存互換性を偽らない。物性を指定しない旧Contentのmanifestは変更しない。

`WorldEdit::integrate_material_forces` は、activeな非空・非blockのcellを既存の確定走査順に列挙し、対象layerの `MaterialMotion` 追従channelへ速度を記録する。重力方向側の隣接cellが未読込なら「空気」と推測せず、その候補の力更新を延期する。容量やProviderの失敗では外側のWorld候補を全てrollbackする。速度の固定小数点Q16と残差は正規channel内に保持し、時間依存の浮動小数点や壁時計へ依存しない。

## 標準式

- 抗力：各軸の速度を`velocity × (65536 - drag_q16)/65536`で減衰させる。0方向への整数除算で負側も処理する。
- 重力：作者が入力した各軸Q16加速度へ素材の重力倍率を適用する。
- 浮力：重力方向の隣接セルが自身より重い液体・気体なら、質量差÷隣接質量を使い、その方向の重力と逆向きへ加速度を加える。
- 摩擦：重力方向に固体・blockが接触するとき、重力に直交する軸の速度を素材の摩擦係数で減衰させる。

この基準式はゲーム用のcell接触物性であり、SI物理や任意形状CCDを保証しない。標準以外の接触、複合流体や連成を作者が実装する場合は `MaterialForceProvider` を渡して標準式を**完全置換**する。providerは不変のWorldから保存済み作者状態を参照できるが、callbackに非保存の時間状態を持たせない。利用するProviderのkey/versionと重力などのProfileは作者の実行設定・保存契約へ含める。標準providerの定義を独自providerへ強制しない。

`WorldEdit::step_material_force_motion` は力積分と既存 `transport_material_motion` のDDA・予約・全物質channel追従を単一World候補で行う。失敗時は力だけ先に適用済みになることがなく、1tickに二重積分を発生させない。正規粒子向けの力積分・広域保存可能Job・dirty/sleep・Runtimeの自動Scheduleは**別途E05残作業**であり、この実装単位でQ04を合格にしない。

## 検証

- `tests/material_forces.rs` は標準重力/抗力/摩擦/浮力、独自providerによる置換、容量rollback、DDAと保存再開の一致、Contentの物性値域とfingerprint差異を検査する。
- `examples/material_forces.rs` は重力0.5 cell/tick²の2tickを途中でWorld保存し、連続実行と再開のcheckpoint一致と移動先cellを確認する。
- `cargo test --locked --test material_forces` と `cargo run --locked --example material_forces` を対象検証する。全体・全OS CIはE05 evidenceへ記録する。

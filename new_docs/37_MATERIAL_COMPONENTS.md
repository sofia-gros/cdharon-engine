# E05：混合物と正規粒子の成分別監査

`World::resident_mixture_inventory(layer, mixture_channel)`は、PR #8の`MixtureStore::quantities`を利用して対象layerの全resident cell・正規粒子の純物質token/massを`MaterialQuantity`の有理数のまま集計する。混合carrierの素材IDを成分量に重複計上しない。

`mixture_channel`は対象layerに追従する`MixtureRef`型で、既定値は`MixtureRef::NONE`が必要。Worldの共有MixtureStoreに実在するindexを照合し、値・coverageが不正なら失敗する。粒子はPR #10の所有する全payloadから混合参照を取り出す。

- 各素材のresident cell由来と粒子由来の成分token、合計、質量を安定した素材キー順に返す。
- 非活動だがresidentの区域も計上する。外部archiveは読まず、退避chunk数と対象layerの退避粒子数を結果へ残す。
- PR #8の有理数量とPR #10の粒子への所有権移動を連携して、混合前後・放出・保存復元・着地で成分総量の一致を公開APIテストで確認する。
- 呼出側は任意のlayerごとに実行できる。後工程のbody/reservoirと外部archive、あらゆる独自provider、全てのsource/sink履歴との照合を自動で認証するものではない。

診断は全resident cellへの型付きchannel queryと組成展開を行うため、毎tickのhot kernelには使用しない。追加のsave版・schema・依存crateは不要。実行結果はE05 evidenceとCIで管理する。E05/Q04は引き続き未完成。

# E05：resident物質量の診断集計

`World::resident_material_inventory()` は全resident chunkの各layerとresident正規物質粒子を安定した素材キー順に合算する。素材のcell token数、粒子token数、それぞれの合計、Contentの`mass_per_cell`による`u128`換算の**名目質量**を公開する。混合cellはPR #8の成分別の有理数量とは別に、carrier素材IDで計上する。

- inactiveだがresidentのchunkも走査する。生成時baseは現在地形と二重に数えない。
- 複数layerで同じ素材を使用した場合は同じ素材へ合算する。空cellは加算しない。block個体は占有cellを数える。
- 粒子に移行したtokenはcellと粒子のどちらか一方にだけ存在するため、放出・着地後も合計が不変となることを検査できる。
- 退避済みchunkの内容はArchiveStorageが保管し、Worldにないため読み取らない。結果の`archived_chunks`と`archived_particles`で未計上の存在を知らせる。
- 混合組成の成分量、作者channel、将来の剛体local cell、reservoir、変換receipt、退避blobの全量突合を含む全表現監査は、**まだ未実装**。このAPIだけでQ04の物質収支を合格にしない。

全resident cellを走査する診断APIであり、毎tick利用する高速集計器ではない。追加の通常保存形式はない。公開戻り値は実行時に再計算し、Worldのcheckpoint/restore前後で一致させる。

検証は`tests/material_inventory.rs`へ置く。inactive chunk・多layer・名目質量・放出着地・checkpoint復帰と、PR #4の輸送・PR #6のreceiptによる意図的な発行・PR #10の退避復帰を同じ集計契約で検査する。E05 3 OS CIへ追加し、既存のPR #5の速度、PR #7の圧力、PR #8の混合、PR #9の押出し、PR #10の粒子の作例も実行し続ける。Rust検査の成否はE05 evidenceとPRのCIに記録する。

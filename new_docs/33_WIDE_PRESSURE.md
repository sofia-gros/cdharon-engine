# 広域圧力と気体の圧力源

E05の圧力計算を分割して進める公開API。工程全体の完成判定は[E05の記録](evidence/E05/README.md)を参照する。

## 作者の使い方

`PressureJob::new(&world, layer, config)`が入力を採取し、`step(&world, budget)`が隣接構築、連結探索、連結量の反映、Jacobi更新と二重bufferの切替をcell単位で進める。各cellの四近傍は上・左・右・下の固定順で、列挙順やstepの分割で結果は変わらない。`completed_work`、`completed_iterations`、`sample_count`、`report`で進捗を診断する。0予算では進めない。各stageの切替も一件として数える。

完成した場合だけ`Ready`を返す。`WorldEdit::publish_pressure_job`が全出力の最終channel容量・編集容量を検査し、一括公開する。既定値に戻す出力を先に反映して空chunkを解放する。途中の圧力・地形は公開しない。失敗をcallbackが無視しても外側のWorld取引を破棄する。

```rust,ignore
let mut job = PressureJob::new(&world, layer, config)?;
job.step(&world, 64)?;
world.edit(|edit| edit.set_state(job_key, job_schema, &job))?;
if job.status() == PressureJobStatus::Ready {
    world.edit(|edit| edit.publish_pressure_job(&job))?;
}
```

schemaは起動時に`Schema::of::<PressureJob>`で登録する。型の保存形式は`CHPRJOB2`。PR初版の`CHPRJOB1`にはlambdaがなく、本版はそのrecordを拒否する。既存保存を読み換える自動migrationは用意せず、作者のschema移行が必要である。独自solverの採用を制限せず、作者は同じchannel・StateStore・取引APIで別の数値方式や分割規則を使える。

## 従来profileとの一致

`PressureJobConfig.gas = None`、`pump = None`では、既存の`relax_pressure`と同じ液体・気体の連結、自由表面0、閉領域の前回平均anchor、massと重力項、Jacobi反復を使う。浮動小数点は使わず、同じ反復数なら分割予算にかかわらず同じ出力になる。標準反復数4は製品の固定制限ではなく、呼出側が`PressureRelaxation`に指定する。

## 気体profile

`GasPressureSource`を渡すと、液体と気体の連結を分離する。気体は同じ活動域の気体・空cellを四近傍で連結し、全素材の気体massと絶対温度Q16を積算する。空cell一つを含む各sampleが容積1である。

`pressure_q16 = floor(Σ(mass_per_cell × temperature_q16) × coefficient_q16 / (volume × 65536))`

例えばmass2・温度3・容積3なら圧力2、容積2なら圧力3となる。温度はi64の1対1channelから読み、位置固定または同layerの物質追従を選べる。温度と出力圧力は別channelを要求する。係数は作者が指定する非負Q16で、0も有効。負温度・負の入力圧・負のポンプ値、計算のu128積上限・出力i64上限を超える値はエラーにする。

得られたtargetへ、各気体cellを近傍圧と重みlambda（`relaxation_weight`）で緩和する。`Pnext = max(0, trunc((Σ(Pneighbor + rho_avg*g*dy) + lambda*target)/(neighbor_count + lambda)))`。lambda=0も有効で、近傍がなくlambda=0なら前回圧を維持する。空cellの圧力は派生境界としてtargetを使い、気体cellの前回圧をtargetへ直接上書きしない。空間のtargetを計算する係数と、targetへ近付く緩和の重みは別設定である。公開するのは占有する流体cellのみで、空cellには気体圧を保存しない。block、固体、粉体、液体、World境界、活動外、未読込は気体空間のno-flow境界。有限Worldの部分chunkの外を容積へ含めない。空だけの空間を流体領域の診断件数へ含めず、温度0の気体領域は数える。

これはcell占有のゲーム用pressure proxyであり、実在流体の精度、異なる容積のtile流体、圧縮性の連続体計算を保証する方式ではない。圧力だけを理由に気体を発行・消去しない。物質移動は既存の輸送・押出しphaseに任せる。温度は作者が与える状態であり、enthalpyからの温度解決はE06で接続する。

## ポンプ境界

`pump = Some(channel_key)`は、別の位置固定・1対1のi64 channelの非負Q16値を各流体cellの最低圧にする。自由表面や気体targetより高い値も明示的に指定でき、隣接する液体への伝播は同じJacobi反復で行う。0なら通常の境界を変えない。圧力設定は仕事量・energy・物質sourceの発行記録ではない。作者が機械の消費・発行を持つ場合は別の理由付き取引で記録する。

## 入力失効と保存再開

World ID・Content fingerprint・material epoch・活動chunk集合とそのrevisionを保持する。入力の占有、channel、活動集合が変われば古いjobを失効させる。無関係な位置の編集でも同じ活動chunkなら再計算が必要な保守的方針である。完成後の公開によるrevision変更で同じ非空jobの二重公開を拒否する。出力がないjobの公開は状態を変えない。

保存には入力と計算中の配列・queue・cursor・反復数を含める。復元直後は完成済みrecordでも`Computing`に戻し、公開を禁止する。新しく採取したWorld入力との一致を確認し、保存時点までの決定的計算をstep予算内で再実行してscratch全体を照合する。照合中は`completed_work`と`completed_iterations`を保存時点に保ち、終わってから残る計算を進める。切断・不正件数・負値・不正index・queue重複・不正段階はdecodeで拒否し、形式上は読めるscratchの改変・入力の欠落も再計算と公開前の照合で拒否する。照合の途中で再保存すると、その一時的な照合計算は保存せず、次の復元で照合をやり直す。

通常のstepはfrontier全体を複製しない。計算中の数値エラーは現在のcell処理直前で止まり、先に終えた仕事はjobに残る。Worldは変更されない。値域を超えた入力を直して同じjobを使うことはできず、変更後のWorldから新しいjobを作る。

## 費用と未統合の範囲

- newの入力採取は活動chunkの全4096cellを走査し、sampleを安定順に並べる。気体profileでは空cellも保持する。`cell_limit`にそれらを含め、容量不足なら開始しない。
- stepの予算はcell処理の件数で、隣接検索にはsample数に対する二分探索を使う。毎stepの活動集合・revision照合は別途chunk数に依存する。wall-clockの一定時間保証ではない。
- scratchはsample配列と最大sample数の探索queue。保存・復元は配列全体に比例する。StateStoreへ毎step保存する費用はsolverの件数予算に含めない。
- 復元時の再検査は独立scratchを追加し、保存時点までの計算量を再度必要とする。計算再実行はstep予算で進むが、再検査開始時の入力採取と最後の全配列比較は一括費用である。
- 公開は入力を再採取するscratch、全入力比較、出力列、channel容量予約と一括書込の費用を持つ。全反復のやり直しは公開時に行わない。

このAPIを単独で全活動域を自動分割するscheduler、dirty/sleep、全solverのprovider登録、力の積分、物質粒子・混合・全表現収支の完成とは扱わない。巨大域で入力採取や最終公開まで厳密な時間分割が必要な作者は別profileを使える。これらのE05統合とQ04は続いて検証する。

## 検証

`tests/pressure_job.rs`で従来profileとの保存byte一致、予算1を含む分割、各計算段階のcodec復元、Worldとjobの同時保存、温度・容積・連結・chunk境界・非活動域・有限境界、ポンプ、lambdaの0・1・3の参照式、入力失効、負値と数値上限、容量不足・無視されたエラーのrollback、切断とscratch改変を検査する。`cargo run --locked --example pressure_job`はmass2・温度3・容積3で圧力Q16=131072を検査し、途中保存と連続実行のcheckpoint一致をassertする。

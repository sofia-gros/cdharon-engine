# E05：速度channelの力provider入口

`WorldEdit::apply_material_forces(layer, channel, provider, candidate_limit)`は、活動中の非block・非空cellだけを安定順で列挙し、`MaterialForceProvider::evaluate`のQ16速度をMaterialMotionに書き込む。未消化端数は変化させない。戻り値はmaterial key/base/mass、座標、tick、現在の速度を含む`MaterialForceInput`から算出する。対応するlayerに追従するMaterialMotion channel以外は拒否する。

- `UniformMaterialForces`は均一なQ16重力、気体浮力、抗力、粉体の水平摩擦を整数で算出する。係数範囲と速度上限を検証し、overrunを無言で丸めない。静止固体を0速度とする。
- 作者の独自providerは同じ型付き速度の公開経路を使う。途中失敗・容量不足・providerの拒否はWorld候補全体のrollback対象で、既存値や先行対象も変化しない。
- `transport_material_motion`と同じWorld::editで組み合わせると力更新・輸送を一括確定できる。RuntimeのX4スケジュール、sim設定とproviderの安定キー/版、他の物性/周辺温度/圧力の完全統合は今後のE05作業である。
- 局所操作であり、継続Job・sleep・熱/反応・熱力学精度を意味しない。E05・Q04の出口条件は変更しない。

`tests/material_motion.rs`へQ16速度と端数、作者の失敗・無視時のrollback、保存再開、channel所属、気体浮力と不正係数の検証を追加する。CIとレビューの実行結果が得られるまで本単位も完了扱いにしない。

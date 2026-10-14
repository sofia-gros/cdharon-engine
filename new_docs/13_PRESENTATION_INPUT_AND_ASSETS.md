# 描画、資産、入力、UI、音響

## 資産のパイプライン

資産は`source -> import -> validated record -> runtime cache -> GPU/audio resource`で管理する。AssetKey、型、importer version、source hash、依存hash、設定、使用許諾情報をmanifestへ記録する。

v1の基準形式は透過PNG、sprite sheetとJSON atlas、WAV/Ogg Vorbis、TTF/OTF、WGSL、JSON prefab/map/localeとする。拡張importerで別形式を追加できる。画像の展開後寸法、byte数、frame範囲、参照先、循環依存をインポート前後で検証する。

`assets.root`は設定ファイルの親から解決する。モジュールの資産はpackage mountを持ち、`package:path`からVFSへ解決する。配布時は同じAssetKeyでpackへ接続し、OSの実行時作業フォルダーへ依存しない。大小文字の衝突と不正なpackage外参照を検出する。

資産はscope leaseと明示保持で寿命を管理する。View用の軽量ハンドルと、資産を生かす保持権を区別する。非同期結果はgenerationを検証し、キャンセル後のSceneへ資産を挿入しない。

## hot reload

画像、音、UI theme、shader等の表示資産は新しい世代を準備し、検証後に境界で差し替える。利用中のGPU資源はfence完了まで保持する。インポート失敗では旧世代を維持する。

衝突mask、material物性、effect program等が変わる場合は正規content変更になる。World停止、移行の検証、manifest更新、必要なネットワーク再合意を経る。見た目の差し替えとして物理を無断変更しない。

## renderer

描画backendはwgpuを基準とし、Worldの不変RenderSnapshotから描画する。Vulkan、D3D12、Metalの認定組み合わせをrelease manifestへ記録する。CPUの物理状態やAPIをGPUの資源へ直接結び付けない。

render graphは背景・parallax、背景壁、地形、body/Entity、物質粒子、視覚粒子、light/occlusion、合成・post effect、UIのpassを持つ。layerとz順を安定化し、作者はpassとshader/materialを追加できる。相互依存が循環したgraphは起動時に拒否する。

標準passの追加だけでなく無効化・置換、独自channel/StateStoreからのsnapshot抽出、custom vertex/index buffer、UI canvasを公開する。独自framing、液量、染色、画面効果やUIを標準spriteの構成へ丸めない。表示cacheをゲーム規則へ逆読取せず、ゲームに使う光量や音刺激は正規Systemへ登録する。[拡張契約](25_OPEN_EXTENSION_AND_COLLISION_CONTRACTS.md)

地形はchunk単位のmaterial/variation/mask textureとpaletteを使い、dirty領域だけ更新する。spriteはatlasとinstancingでbatchし、破壊可能bodyのlocal gridも同じ素材表示へ接続する。viewからGPUへのupload byte数と同期回数を計測する。

alpha、色空間、premultiplied alpha、filterをimporter recordへ固定する。pixel-art用のnearestと整数倍率、通常のlinear表示、letterbox、UI倍率を選べる。画面・viewport・camera・Worldの座標変換は一つの変換APIを使う。

## 光と液体表現

v1は色付きの動的照明、素材ごとの遮光・発光、地形変更による影、bodyの遮蔽を提供する。基準light solverは格子の減衰伝播とsource queue、dirty更新、安定した反復数を持つ。GPU版は表示用の近似として同じ入力を受ける。

液体のsurface、屈折、発光、熱や衝撃の画面効果、bloom等はrender graphの選択可能なpassとする。光の品質を下げても正規の視界query、damage、熱が変わらない。

hardware ray tracingはv1の必須backendにせず、描画機能の能力と要求を表現する拡張点を残す。これに依存せず、上の照明機能と二つの検証作品の見た目が成立することを出荷条件とする。

## カメラとアニメーション

複数camera、viewport、追従、zoom、shake、背景parallax、Sceneの重ね合わせを提供する。Transformの親子関係とanimationのtimelineを使い、sprite frame、装備anchor、event markerを制御する。

物理の補間は表示に限定する。gameplayに使うanimation markerはtick側で評価し、可変の表示frameを命中時刻の正にしない。UI用や装飾用のtimelineは表示時計を使える。

## 入力

device inputをtimestamp付きで取得し、対象tickへ量子化する。アクションはbutton、axis、pointerを型別に登録し、keyboard、mouse、gamepad、複数のlocal playerへ割り当てる。再割り当て、deadzone、感度、競合診断、設定の保存を提供する。

`held`、`just_pressed`、`just_released`を区別し、描画頻度に依存しないedge消費を行う。UIからの入力消費、Sceneのpriority、focus、modalを解決してからゲームへ送る。ポインターはcameraを逆変換し、表示領域外を有効なWorld座標へ丸めない。

focus喪失、gamepad切断、IME使用時の扱いを定義する。オンラインのWorldをOS focusだけで停止させず、local入力を中立へ戻す。

## UIと文字

保持型のUI treeを提供し、layout、clip、scroll、focus、button、text、image、input field、list、grid、drag/dropを備える。inventoryやhotbar、map、設定画面を公開部品から制作できる。

layoutはTaffy、文字組みはcosmic-textをadapter経由で利用する方針とする。日本語・結合文字・双方向文字・font fallback・IMEを扱い、locale key、数値表示、キー割り当て表示を提供する。文字列長を描画上の文字数と混同しない。

UI倍率、色以外の状態表示、字幕、点滅・shake軽減、音量個別調整を用意する。screen readerへ公開するaccessibility treeとkeyboard操作を認定環境で確認する。UIはWorldの可変配列を直接保持せず、queryとcommandを使う。

## 音

音響backendはKiraをadapter経由で利用し、voice、bus、音楽、streaming、spatial sound、fade、音量を提供する。音声threadではWorldへアクセスせず、準備済みデータと短いcommandだけを処理する。

確定イベントには`world, tick, source, sequence`のEventIdを付け、replayやnetwork再送で同じ効果音を二重再生しない。連続音はvoice keyで追跡し、Scene/Worldのscopeと連動して停止する。正規の機械状態から復元後の連続音を再接続する。

遮蔽と距離減衰は表示用queryとして計算し、音声deviceの有無でゲーム規則が変わらない。voice上限時はpriorityの低い演出を明示方針で抑制し、world item等の正規出力を減らさない。

## デバイス障害

surface resize、GPU device lost、音声device切断では表示側の世代を破棄し、CPU側の資産から再生成する。失敗した場合もWorldと保存機能を維持する。GPUの待機中に未完了のSnapshotを編集しない。

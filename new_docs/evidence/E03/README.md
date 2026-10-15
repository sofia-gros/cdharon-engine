# E03：資産と基本表示の実装・検証記録

## 対象

基準revisionはE02の`e97396c`。[工程表](../../19_IMPLEMENTATION_ROADMAP.md)のE03として、VFS、pack、import、cache、Scene共有・解放・差し替え、PNG/音/font、sprite、camera、render adapterを実装した。旧M3等の既存未コミット変更を含めず、新基盤の公開APIで動く経路を検査する。

契約と作者向けの説明は[28_ASSET_AND_BASIC_RENDERING](../../28_ASSET_AND_BASIC_RENDERING.md)、実行例は[assets_scene.rs](../../../examples/assets_scene.rs)。v1全体、E09の完成した表現・UI・音声再生、E13の性能・全機器認定が完了したという意味ではない。

## 実装したもの

| source | 内容 |
| --- | --- |
| [assets/vfs.rs](../../../src/assets/vfs.rs) | Unicode NFCの仮想path、directory/pack mount、境界検査、上限付き読取、SHA-256、CHPK01 |
| [assets/import.rs](../../../src/assets/import.rs) | PNGのRGBA8/sRGB変換、sheet、JSON atlas、WAV/Ogg PCM、TTF/OTF、独自Asset/Importer |
| [assets/mod.rs](../../../src/assets/mod.rs) | 型付きキー・handle・lease、世代・cache・scope、worker、取消、完了待ちを含む容量、manifest |
| [presentation/mod.rs](../../../src/presentation/mod.rs) | sprite抽出、複数camera、順序・変換・clip、参照renderer、adapter置換、surface座標変換 |
| [runtime/scene.rs](../../../src/runtime/scene.rs) | LoadContextの資産loader、FrameContextのdraw/reload、Scene scopeとの接続 |
| [runtime/engine.rs](../../../src/runtime/engine.rs) | 作者AssetServer/RenderAdapterの受取、既定root mount、frame抽出、停止時の保持権解除 |
| [runtime/host.rs](../../../src/runtime/host.rs) | pixels/wgpuによるGPU表示、実adapter情報、resize、fit/integer、表示上限と固定時計の分離 |
| [tests/assets.rs](../../../tests/assets.rs) | E03の公開API試験24件 |
| [fixture](../../../tests/fixtures/e03/README.md) | 同じpack・PNG・WAV・Ogg・atlas・OFL fontを各OSへ渡す固定データ |

既存の特定ゲーム向け`render.rs`や`core::engine`へ逆依存しない。Worldの保存・simulation形式は変更していない。公開APIは`assets`と`presentation`を追加し、LoadContext/FrameContextを拡張した。設定schema 1へ互換な`render.scaling: fit/integer`を追加し、[設定文書](../../05_ENGINE_CONFIGURATION.md)も更新した。

## 依存

Cargo.lockで実際の版を固定する。自作decoderやhashへ置き換えるより、保守される専用実装を使って外部データを解析する。

| 直接依存 | 用途・選択理由 |
| --- | --- |
| png 0.18 | 寸法・CRC・展開量の制限を使うPNG codec。画像全般を含む大きな依存群を導入しない |
| serde_json 1 | atlasの型付きschema。未知fieldと重複frameを拒否 |
| sha2 0.10 | packの完全性とsource/import/依存の正規hash |
| hound 3.5 | PCM/float WAVの読取。音声deviceを要求しない |
| lewton 0.10 | RustのOgg Vorbis decode。OS codecへ依存しない |
| ttf-parser 0.25 | TTF/OTFを借用で検証。font installや自己参照型を不要にする |
| unicode-normalization 0.1 | NFC/NFDが異なるOSでも同じ仮想名を使う |

GPUは既存のpixels/wgpu、windowは既存のwinitを使う。音声再生・文字組みのためにKiraやcosmic-textを導入したという主張はしない。font fixtureはGoogle FontsのAbelで、SIL OFL 1.1を同梱した。検査用画像・音・atlasは本プロジェクトで生成し、使用許諾をfixture READMEへ記録した。

## 検証した不変条件

- packは名前順で同じbyte列になり、全切断位置・破損hash・巨大長さ・大小文字衝突を拒否する。directoryから読んだsourceとpackのsourceが一致する。
- 仮想名はNFCへ正規化し、path traversal・絶対参照・Windows予約名を全OSで同じように拒否する。
- PNGは透過・palette・gamma・sheetのframe寸法を検査する。alphaへgammaを適用せず、sourceの透明な色がlinear filterで縁へ混ざらない。
- 音とfontをwindow/deviceなしでimportできる。fontの実glyphとOggの実PCMを確認する。
- 複数Sceneは同じ資産を共有し、Scene停止でscopeの保持権を解除する。明示leaseと提出済みframeは必要な旧資産を保持する。
- 旧世代のhandle、別cacheのhandle、型違い、同じキーの別定義を拒否する。reload失敗で旧世代を残す。
- scope終了・取消・後発reload後に、遅い結果を挿入しない。要求数・展開量・完了待ち結果・外部leaseを上限へ含める。
- cameraの順逆変換、viewport外、letterbox、整数表示時のclipを確認する。cameraを変えてWorldやtickを変更しない。
- layer/z/提出順、alpha、clip、複数camera、linear filterをCPU参照経路で検査する。draw失敗を無視してもそのframeを表示しない。
- Scene::load→FrameContext::draw→Engine::render_intoが実際のAssetServerへ接続する。shutdownで最後のframeとscopeの保持を回収する。

## レビューと修正

| 発見した問題 | 修正 |
| --- | --- |
| workerの完了待ち結果がcache予算外になる | 準備中からBudgetPermitを持ち、公開後も同じ予算をresidentへ移す |
| 待ち行列だけの制限では未回収jobを増やせる | 未回収要求数も別のpermitで制限し、回収・取消・dropで解除 |
| Sceneが未回収jobを保持すると停止後も結果と要求枠が残る | scope取消とjob取消で結果・要求枠を回収し、送信との競合もworker側で回収。別cacheへの誤pollは元のjobを消費しない |
| 停止後も古いscopeへ完了を返せる | scopeごとの取消flagと公開前のscope・cache・世代検証 |
| reload前のscope保持が同じキーで積み重なる | そのscopeの旧世代保持を新世代へ置換。明示leaseと別scopeの保持は維持 |
| PNGの古いsource_gamma fieldではgAMAを取得できない | 現行decoderのgamma()/chromaticities()を使い、linear gray 128→sRGB 188を試験 |
| atlasのBTreeMapが重複名を上書きする | serde visitorで重複を拒否 |
| 依存source hashだけではimport設定の違いを捉えられない | manifest全体のcontent_hashを依存へ記録 |
| drawエラーを無視すると部分frameが残る | 最初の提出失敗を保持し、参照rendererで公開拒否 |
| 極端な画像・画面寸法のRGBA byte数がoverflowする | 各乗算とusize変換を検査し、u32最大寸法の境界テストでpanicを防止 |
| OS間の名前正規化、surfaceの余白・clipが異なる | NFCの仮想名と共通のSurfaceMappingを追加 |

## ローカル実行結果（2026-10-15、Windows x86_64 MSVC）

| 実行 | 結果 |
| --- | --- |
| `cargo fmt --all -- --check` | 成功 |
| `cargo test --locked --target-dir target/e01-validation -j 1 --quiet` | 全177件成功：既存112 + E01 19 + E02 24 + E03 22 |
| `cargo check --all-targets --locked --target-dir target/e01-validation -j 1` | 成功 |
| `cargo clippy --all-targets --locked --target-dir target/e01-validation -j 1 -- -D warnings` | 成功、警告なし |
| `cargo doc --lib --no-deps --locked --target-dir target/e01-validation -j 1` | 成功 |
| `cargo run --locked --example assets_scene --target-dir target/e01-validation -j 1` | packから全形式を読み、gallery.pngを出力。画像も目視確認 |
| 同コマンドの`-- --window-smoke` | Vulkan / NVIDIA GeForce RTX 4060で実表示。120固定tick、最終実行275表示frame、exit code 0 |

新しいtargetディレクトリでの依存再構築ではページングfile不足が発生したため、その失敗を成功として扱わず、既存の依存cacheを用いて直列検査した。異なる作業コピーへ移る場合はpackageの生成物を分離・再構築し、旧metadataを混用しない。

## 分離検査とOS

コミット予定のtreeを別の作業コピーへ展開して検査する。旧M3等の未コミット変更を含まない構成では既存が101件となるため、E01 19 + E02 24 + E03 22を加えた**166件**が対象になる。作業ツリーの177件とは区別する。

[E03 CI](../../../.github/workflows/e03-assets.yml)はWindows、Ubuntu、macOSで同じ固定packの読込、各形式のimport、scope/世代/取消、CPU参照描画、headless作例を実行する。初回コミット`286f6f4`は[3 OS CI](https://github.com/sofia-gros/cdharon-engine/actions/runs/38660153737)と[Rust CI](https://github.com/sofia-gros/cdharon-engine/actions/runs/38660153589)で成功した。取消回収の追加修正は再検証する。Windows実機以外のGPU/surface認定を、このheadless CIの成功だけで主張しない。

## 後工程の範囲

E04がmaterial/chunk/保存へ、E09がlight/shadow/render graph/UI/文字組み/音声再生・機器復帰へ接続する。E03でRenderAdapterを登録できることを、未実装のsolverや表現の完成とは数えない。shader、geometry、通信等の置換能力は担当工程の実装とfixtureで検証する。

標準PNG importerのICC・広色域・APNG、UI animation、Ogg streaming再生等は専用のimporter/表示・音響サービスへ接続する。静止画importerが未対応の内容を成功扱いで捨てることはしない。同期loadの待機、scalar rasterize、GPUへの全frame uploadは費用を明示し、E13の性能認定を先取りしない。

### コミット対象の最終ローカル検査

indexから展開した`target/e03-review-tree`で、境界修正後の全166件、fmt、全target check、警告拒否Clippy、Rustdocが成功した。最終コードのOS window smokeもVulkan / GeForce RTX 4060で成功し、120固定tickの間に285表示frameを実行した。旧M3等の変更はこの検査とコミットに含めていない。

### 取消回収の追加修正

Sceneがjobを保持したまま停止する場合と、明示cancel後もjobを保持する場合を追加した。完了済み結果のbytesが0へ戻り、jobs=1でも次の要求を受理できることを確認した。別cacheへの誤pollが元の要求を失わせない試験も加えた。

最終の分離作業コピーで全**168件**（既存101 + E01 19 + E02 24 + E03 24）、fmt、全target check、警告拒否Clippy、Rustdocが成功した。上の177件・166件は追加修正前の実行履歴であり、今回の最終件数とは区別する。

追加修正後のOS window smokeもVulkan / GeForce RTX 4060で成功した。120固定tick、273表示frame、exit code 0を確認した。

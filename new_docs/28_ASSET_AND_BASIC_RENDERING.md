# E03：資産と基本描画の公開契約

## 位置付け

E03は、作者のファイルを検証し、Sceneの寿命へ接続し、spriteとcameraを使って表示する工程である。実装と実行結果は[evidence/E03](evidence/E03/README.md)、動く利用例は[assets_scene.rs](../examples/assets_scene.rs)、公開API試験は[tests/assets.rs](../tests/assets.rs)にある。

光・影・post effect・保持型UI・IME・文字組み・音声再生はE09へつながる別の機能である。fontを読めたことと日本語UIを組めたこと、音をdecodeしたことと音声deviceで再生できたことを混同しない。

## 作者の入口

```rust
use charon_engine::{
    assets::{AssetHandle, ImageAsset},
    foundation::EngineResult,
    presentation::Sprite,
    runtime::{FrameContext, LoadContext, Scene},
};

#[derive(Default)]
struct TitleScene {
    background: Option<AssetHandle<ImageAsset>>,
}

impl Scene for TitleScene {
    fn load(&mut self, ctx: &mut LoadContext<'_>) -> EngineResult<()> {
        // assets.root内の画像を読み、このSceneの開始期間に保持します。
        self.background = Some(ctx.assets().image("background", "images/title.png")?);
        Ok(())
    }

    fn render(&mut self, ctx: &mut FrameContext<'_>) -> EngineResult<()> {
        if let Some(image) = self.background {
            // 表示寸法は作者コードが決め、画像や物理形状は変更しません。
            ctx.draw().image(image, Sprite::new().size(1280.0, 720.0))?;
        }
        Ok(())
    }
}
```

`image`はPNG、`spritesheet`は等間隔frame、`atlas`は名前付き矩形、`audio`はWAV/Ogg、`font`はTTF/OTFを読み込む。通常の作者用APIは短いキーと相対sourceを`game` packageで修飾する。例えば`images/title.png`はVFSの`game:images/title.png`となり、OSの実行時作業フォルダへ直接参照しない。別packageを使う場合は完全名を渡す。

`EngineBuilder::asset_server`で配布packをmountしたserverを渡せる。未指定の場合、存在する`assets.root`を`game` packageとして読み取り専用mountする。空のゲームは資産フォルダなしでも起動できるが、存在しない画像のloadが成功するわけではない。

## VFSと配布pack

- directoryとpackは同じ`VirtualPath`を使う。mountは検査後に一括反映し、既存名を暗黙に上書きしない。
- 名前はUnicode NFCへ正規化する。大小文字だけが違う名前、正規化後の重複、親参照、絶対path、空成分、Windowsの予約名、不正な区切り文字を全OSで拒否する。
- directoryは登録時と読取時にcanonical pathを検査する。リンクや外部への参照を資産として登録しない。ファイル数、source byte数、pack byte数は呼出側の上限を使う。
- VFSのcloneはmount一覧の`Arc`を共有する。workerごとに全ファイル一覧を複製しない。mountの変更時はcopy on writeで別snapshotを作る。
- `CHPK01`は圧縮しない形式であり、展開bombを持たない。SHA-256は破損の検出に使い、配布者の認証とは扱わない。

| CHPK01のfield | 形式 |
| --- | --- |
| 形式識別 | 6byteの`CHPK01` |
| ファイル数 | little-endian u32 |
| 各ファイルの仮想名 | u32 byte長 + UTF-8。完全名のbyte順で記録 |
| source hash | SHA-256の32byte |
| source | u32 byte長 + 生byte |

読取時は入力全体・個別長さ・名前・checksum・余分な末尾を確認する。同じ入力集合からのpackは入力順によらず同じbyte列になる。認定試験は[固定pack](../tests/fixtures/e03/assets.chpack)を使う。pack生成も`AssetPack::new`と`encode`という公開APIで行える。

## importとmanifest

標準画像はRGBA8、straight alpha、sRGBへ変換する。paletteとtRNS、grayscale、16bit sourceを扱い、寸法・CRC・展開量・frame範囲を検証する。gAMAがある場合はRGBをsRGBへ変換し、alphaへgammaをかけない。sRGB chunkを優先する。ICCや非sRGB原色、APNGは標準の静止画importerで誤って解釈せず、専用importerを必要とする。

WAV/Ogg Vorbisはinterleaved f32 PCMへdecodeする。sample rate、channel数、PCM値域、展開上限を確認する。fontは元byteとface番号を保持し、必要時に借用した`ttf_parser::Face`を作る。OSのfont installや音声機器はimportの前提にしない。

JSON atlasの標準schemaは次である。別の制作ツールの形式は`Importer<AtlasAsset>`で接続できる。画像の回転・trim等を未対応のまま黙って捨てない。

```json
{
  "schema_version": 1,
  "frames": {
    "idle": { "x": 0, "y": 0, "width": 16, "height": 16 }
  }
}
```

frame名の重複、未知field、ゼロ寸法、overflow、画像外の矩形を拒否する。atlasは画像の特定世代のleaseを持つ。画像の差し替えだけでatlasの参照を勝手に新世代へつなぎ直さず、atlasも検証して再importする。

`AssetManifest`は安定キー、source、source hash、importerの版と設定、依存のcontent hash、使用許諾情報を記録する。`content_hash()`はこれら全体の正規hashであり、sourceが同じでもimport設定が違えば区別できる。`set_source_license`は以後のimportへ使用許諾情報を付ける。未指定を許諾済みとは表示しない。

作者の独自型は`Asset`、変換処理は`Importer<T>`を実装する。`validate`、`resident_bytes`、importer identity、依存hashも同じ経路へ参加する。独自型の実装者は保持量と検証の契約に責任を持ち、正規のWorld状態を資産cacheに隠さない。

## 保持・世代・容量

| 型 | 意味 |
| --- | --- |
| `AssetKey<T>` | 型付きの論理名。未読込でも作れる |
| `AssetHandle<T>` | cache識別子・キー・世代を持つ軽い参照。保持しない |
| `AssetLease<T>` | 特定世代を生存させる所有権。cloneで明示保持を増やせる |
| Scene scope | loadした資産のleaseをまとめて持つ開始期間 |
| `SpriteCommand` | frameが使う画像のleaseを保持する描画snapshot |

同じ定義を複数Sceneでloadした場合は同じCPU資産を共有する。Scene停止でscopeの保持権と要求を解除する。別Scene、明示lease、提出済みframeが保持している資産は残る。再loadは世代を進め、古いhandleを別の実体へ向け直さない。旧leaseは旧内容を引き続き読める。

cacheは未使用資産を回収でき、使用中資産を上限のために強制破棄しない。差し替えは新世代の準備に成功してから公開するため、旧世代と新世代が同時に生存できる予算が必要になる。

未回収job数・worker数・source長・展開量・cache保持量・キー数・scope保持数を制限する。workerが準備中の結果と完了待ち結果にもbyte予算を持たせ、公開後は同じ予算をresidentへ移す。旧世代の外部leaseも計上する。cache byte数はCPU資産の保持配列を対象とし、source pack、metadata、codecの一時buffer、thread、GPU uploadは別の費用として測る。process全体のRAM上限や性能認定はE13の測定対象である。

## 同期loadと非同期reload

`load`はworkerへdecodeを委ね、そのcallback内で完了を待つ。初期化に便利な同期APIであり、安価なsetterとは扱わない。大きい資産を動作中に切り替える作者は`request`を使い、表示側で`poll`する。

```rust
// Sceneの表示用stateとしてjobを保持します。正規Worldの進捗を完了順へ依存させません。
let mut job = ctx.loader().request(
    AssetKey::<ImageAsset>::new("background")?,
    "images/new-background.png",
    PngImporter::default(),
)?;

// 後のFrameContextから明示的に公開します。未完了ならNoneです。
if let Some(handle) = ctx.assets().poll(&mut job)? {
    self.background = Some(handle);
}
```

scope、cache、要求時の資産世代を公開前に照合する。先行する別のreloadが既に成功した場合はConflictとなる。取消、drop、Scene停止では遅い完了を挿入しない。cancelはcodecの強制thread中断ではなく公開権の取消であり、decodeの終了処理は続く。AssetServerのdropはworkerをjoinする。

## spriteとcameraと描画adapter

spriteは位置、表示寸法、anchor、回転、反転、tint/alpha、frame、layer/z、filterを持つ。source画像のpixel数やWorldの衝突形状をsetterで変更しない。layer、z、提出順で安定した描画順を作る。複数Sceneと複数cameraが同じframeへ提出できる。

`Camera2d`はWorld位置・zoom・回転・viewportを持つ。`world_to_screen`と`screen_to_world`は同じ逆変換を使う。viewport外のpointerはNoneとし、World座標へ丸めない。OSの物理pixelから論理frameへの変換は`SurfaceMapping`、そこからWorldへの変換はcameraを使う。

`render.scaling: fit`は縦横比を維持してsurfaceへ配置し、`integer`は整数倍率で余白を付ける。surfaceが小さいinteger表示は等倍の中央clipとなる。HiDPI・resize後も論理cameraとWorldを変えない。

`FrameContext::draw`は通常のscreen camera、`draw_with`は作者cameraを使う。抽出先`RenderFrame`は画像のleaseだけを保持し、Worldの可変参照を持たない。提出時の失敗を無視しても不完全なframeを表示しない。

`RenderAdapter`はsnapshotとRGBA targetを受ける。`EngineBuilder::render_adapter`で標準処理を置換でき、`Engine::render_frame`で独自hostや描画経路へ抽出できる。標準`ScalarRenderer`は正しさを確認する参照経路として、nearest、premultiplied linear補間、linear色でのalpha合成、矩形clip、回転を実装する。sample仕事量の上限を先に検査し、途中の表示を成功扱いにしない。

標準OS hostはCPU側の結果をpixels/wgpuへuploadしてsurfaceへ表示する。GPU処理の結果をWorldへ逆読取しない。実際のadapterは`DesktopGpu` serviceで確認できる。spriteのGPU instancing、render graph、照明等の最適化と完成した表現は担当工程へ接続する。E03の参照経路を、10,000sprite等の性能認定の代わりにはしない。

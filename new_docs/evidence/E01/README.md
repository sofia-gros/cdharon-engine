# フェーズ1 / E01：Coreとデータの実装記録

## 対象

ユーザーの「フェーズ1実装」を、[工程表](../../19_IMPLEMENTATION_ROADMAP.md)のE01として着手した。P001の独自状態・別解像度・候補とrollbackも含める。旧M工程の判定を引き継がず、E00全体の通過をこの記録だけで主張しない。

基準source revisionは`628b96ac9e8613bd386a55b05cc94ad39993596f`。変更はその上の未コミット作業であり、開始前から存在するauthoring、game/session、inventory、World等の変更を保持した。旧文書は開発の根拠として参照していない。

## 追加した公開基盤

| source | 実装した契約 |
| --- | --- |
| [mod.rs](../../../src/foundation/mod.rs) | `charon_engine::foundation`。OS・描画・既存のPlayerに依存しない入口 |
| [error.rs](../../../src/foundation/error.rs) | 失敗のcode、段階、対象、field、必要量/容量、再試行可否 |
| [key.rs](../../../src/foundation/key.rs) | 検証済み`package:name`、短縮キーの修飾、確保不要のキー複製 |
| [id.rs](../../../src/foundation/id.rs) | 型別ID、世代付きarena、空き順と退役を含むcheckpoint |
| [codec.rs](../../../src/foundation/codec.rs) | 上限付きlittle-endian、長さ確認、作者StateCodec、有限floatのbit保持 |
| [schema.rs](../../../src/foundation/schema.rs) | 安定キー/版/型/byte上限、payload検証、明示migration、登録順非依存 |
| [component.rs](../../../src/foundation/component.rs) | World所属付きEntityRef、EntityPool、sparse-set、世代検証、安定順保存 |
| [grid.rs](../../../src/foundation/grid.rs) | 型付きchannel、有理数の格子比、負座標、64×64 dense chunk、疎な確保 |
| [random.rs](../../../src/foundation/random.rs) | 作者RandomSource、保存可能なSplitMix64、偏りを除く範囲乱数 |
| [state.rs](../../../src/foundation/state.rs) | scopeとschemaを持つStateStore、候補、失敗後のcommit拒否、全record復元 |
| [transaction.rs](../../../src/foundation/transaction.rs) | 異種状態の候補、失敗保持、prepare/commit、公開後の旧状態回収 |
| [units.rs](../../../src/foundation/units.rs) | 連続座標・速度・tick、半開矩形、非有限値と端点・面積のoverflow検証 |

[公開APIの利用例](../../../examples/foundation_data.rs)は、作者の生産数・乱数・道具component・中間液量を保存する。Sceneや既存の特定ゲームを起動する必要がない。StateStoreとGridChannelを同じAtomicStateへ入れ、複合する変更を一括公開する例も含む。

## 重要な不変条件

- IDの再利用で世代を巻き戻さず、最大世代から削除したslotは退役する。EntityRefはWorld所属も確認する。
- 未登録schema、型違い、未知の新しい版、移行の欠落、巨大な長さを拒否する。TypeIdと関数pointerは保存しない。
- 保存はallocatorのfree-list、schema識別、scope、revision、grid変換、PRNG stateを含む。復元容量は呼出側が渡し、保存fileが引き上げられない。
- Transactionのput/removeまたはAtomicEditのapplyが失敗を返した場合は、エラーを無視しても先行変更を公開できない。作者のapply内部では失敗を`?`で返し、意図した代替処理だけを成功として返す。prepare後のcommitで確保・codec・検証を呼ばない。
- 複合commitでは旧状態のDropも実行せず、CommitReceiptへ所有権を移す。作者型の解放は公開後の安全な境界で行う。
- GridValueの同一性は正規の保存表現に合わせる。f32/f64の+0と-0を省略判定で混同しない。

## 既存コードの再利用判断

| 既存領域 | 現状の証拠と判断 |
| --- | --- |
| `simulation/entity_id.rs` | 単一種類のEntityAllocator。再利用順と退役の考え方を採用し、型別ID・一般の値arena・外部上限付き復元を新基盤へ実装 |
| `simulation/determinism.rs` | PhiloxとFNVの既存処理を保持。新基盤は状態付き乱数も公開し、標準counter方式への強制を避ける |
| `simulation/storage.rs` | Player/GameSessionに結合した既存世代保存。低水準codecとして流用せず、World保存の移行をE04以降で判断 |
| `simulation/command.rs` | 現行有限Worldの具体的な編集command。一般の作者状態に対する原子性は新しい候補とprepare/commitで実装 |
| `core/engine.rs` | winit、pixels、Playerと現行Worldを直接統合。新基盤へ逆依存させず、Scene/Runtime化はE02以降 |
| `game/session.rs`等 | 作者のゲーム内容と汎用基盤を分離する対象。今回その未コミット変更を上書きしない |

Cargo.toml/Cargo.lockの依存追加・更新はない。schema、疎な配列、codec、候補を標準ライブラリで実装した。既存保存と新しい`CHR*01`基盤recordの互換を自動的に保証せず、外部World保存の世代公開・移行はE04の契約で扱う。

## 性能と接続の範囲

候補の複製は正しい参照経路を優先し、型付き配列/payloadを独立bufferへ複製する。全世界の毎tickコピーを通常APIへ隠す実装ではない。COWやpage単位予約への最適化は同じ原子性を保って別途測定する。独立候補と旧状態回収分のメモリを予算へ含める。

この段階のscopeはStateStore/AtomicStateの安定キーであり、Player/World/Account/Run/providerごとに別storeを構成できる。各scopeの実際の寿命、Worldを跨ぐ共通世代manifest、providerの起動・slot管理は上位Runtime/保存工程で接続する。グリッドの未確保chunkは既定値の意味であり、Worldの未読込判定の代用にはしない。

field単位のデータpack編集・derive・inspector、Scene統合、実際の物理backend、renderer、ネットワーク、原作との全内容照合は、この基盤だけで実装済みとは扱わない。schemaにおける現時点の型・値域・参照検証は、作者がStateCodecとCandidateStateで明示する。

## 検証環境と結果

- 実行日：2026-10-15。
- host：`x86_64-pc-windows-msvc`。
- Rust：`1.98.1 (48a229cea 2026-09-01)`、Cargo：`1.98.1`。
- 試験source：[tests/foundation.rs](../../../tests/foundation.rs)。公開APIだけで失敗時の不変性・保存と再開を検査する。

共有`target/`での最初の試験は、依存rlibが見つからないE0463で失敗した。この結果を合格記録に含めない。別のCargoプロセスが存在していたため停止や削除をせず、`target/e01-validation`へ分離して再ビルドした。以降の最終結果を下表に記録する。

| コマンド | 結果 |
| --- | --- |
| `cargo fmt --all -- --check` | 成功 |
| `cargo check --all-targets --locked` | 成功 |
| `cargo clippy --all-targets --locked -- -D warnings` | 最終変更後も成功。警告0件 |
| `cargo doc --lib --no-deps --locked` | 最終変更後も成功。`target/doc/charon_engine/index.html`を生成 |
| `cargo test --locked --test foundation --target-dir target/e01-validation -j 2` | 初回の新規14件が成功。その後、境界・旧状態解放・batch失敗の試験を追加 |
| `cargo test --locked --target-dir target/e01-validation -j 2 --quiet` | 最終変更後に成功。既存112件、新規19件、合計131件、失敗0件 |
| `cargo run --locked --example foundation_data --target-dir target/e01-validation -j 2` | 成功。生産数16、道具の残量50、液量126を出力 |

## 自己レビューと修正

| 指摘 | 対応と証拠 |
| --- | --- |
| エラーへ193byteのキーを直接持たせるとResult全体が大きくなる | 既に所有されるキーへの共有参照を診断へ保持し、Clippyの警告抑制なしで解消。確保失敗のエラー自体は確保不要 |
| 候補で先行書込後に失敗してもcommitできる | 失敗を保持するTransaction/AtomicEdit。後続の公開を拒否する試験 |
| floatのPartialEqでは正負の0を同一扱いして情報を落とす | GridValueの正規同一性、floatのbit比較、保存と復元の試験 |
| 一般のTを代入するとcommit中に旧TのDropを呼ぶ | mem::replaceとCommitReceiptで公開後へ回収を移す。Drop時刻の試験 |
| dense配列の内部順序を保存・作者の更新順へ漏らす | ID順のordered queryと正規checkpoint。挿入順を逆転した比較試験 |
| 作者kernelの高速な連続書込経路が不足 | GridChannel::edit_chunk。callback失敗、不正値、容量不足では元状態を維持 |
| 保存に書かれた巨大なcapacity/countを信用する危険 | 外部上限、入力の最低byte数、長さ・順序・free-listの重複を検証 |

検証はこの作業コピーに対する記録で、commit/pushは未実行。E00全体の監査、後工程の接続、商用出荷条件へ通過を拡張しない。

E01の実装・試験・レビュー記録を、未完了の後工程や商用v1全体の完成判定へ読み替えない。

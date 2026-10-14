# M0 現状ベースライン

採取日: 2026-10-14。対象はこのcheckoutのprototypeで、製品ゲート合格の証明ではない。

## 開発環境

| 項目 | 観測値 |
| --- | --- |
| OS / target | Windows 10 Pro 26H2 (build 26300) / `x86_64-pc-windows-msvc` |
| Rust | `rustc 1.98.1 (48a229cea 2026-09-01)` |
| Cargo | `cargo 1.98.1 (797e8a9bc 2026-08-05)` |
| Build profile | debug, locked dependencies |
| CPU | AMD Ryzen 7 2700 Eight-Core Processor、16 logical processors |
| RAM | 31.9 GiB total、採取時17.9 GiB available |
| GPU / driver | NVIDIA GeForce RTX 4060 / `32.0.16.1742`; Parsec Virtual Display Adapter / `0.41.0.0` |
| Git working tree | 作業開始時はclean |

この記録から性能や最低動作環境を主張しない。RAMのavailable値は採取時の一時値。

HardwareはPowerShellから取得した。`Get-CimInstance`/`Get-ComputerInfo`は実行環境でCIMアクセス拒否となり、`systeminfo.exe`も資格情報エラーになったため、OS/CPU/GPUはレジストリ、RAMは`GlobalMemoryStatusEx`で読み取った。

## Checkとsample

| Command | 結果 |
| --- | --- |
| `cargo fmt --all -- --check` | pass。sandbox内ではWindowsのアクセス拒否、sandbox外で同じcheckを再実行してpass |
| `cargo check --locked --jobs 1` | pass。Windows path canonicalizationとincremental hard-linkの警告あり |
| `cargo test --locked --jobs 1` | pass。72 passed / 0 failed。TEMP/TMPを`target/m0-temp`へ指定 |
| `cargo run --locked --jobs 1 --bin headless -- 600` | pass。6,144 cells、600 ticks、fingerprint `a0ed97eb260d16f9`、debug elapsed 2,250 ms |
| `cargo run --locked --jobs 1 --bin gameplay_sample` | pass。tick 1、inventory_blades 1、enemies 1、save_bytes 33,004 |

最初の並列check/testではsandboxのプロセス・メモリquotaにより依存crate compilerが異常終了した。直列実行でcheckは通過。初回testは既定TEMPへの書き込みが拒否されstorage test 5件が失敗したが、workspace内の一時領域へ向けた再実行では全件通過した。これはsandbox環境の制約として記録し、製品のstorage failureとは扱わない。

headless実行は小規模な地形生成worldでの動作確認であり、B0の96 active chunks条件を満たさない。elapsedは単一debug runで、性能比較用の統計ではない。

## Baseline sceneと失敗scene

測定用runnerは[`src/bin/m0_baseline.rs`](../../src/bin/m0_baseline.rs)。`cargo run --locked --bin m0_baseline -- all 120`で4 sceneを順に作り、120 tickを進める。各fixtureは64×64、1 active/resident chunk、debug build。2回実行し、全fingerprintの一致を確認した。

| Scene | 現状 | 不足 |
| --- | --- | --- |
| B0 静止 | 全空セルの静止world。run1 298 ms / run2 290 ms、fingerprint `770027dbd34d9f1f` | 製品負荷の96 chunksではない。memory/alloc計測なし |
| B1 流体 | 8×8の水/砂パッチ。333 ms / 332 ms、fingerprint `d9b212ee0f05c54b` | 96 chunks/30%密度/16 entities条件ではない。entity統合測定なし |
| B2 破壊 | 石の塊と床、60tickごとに半径8を除去。297 ms / 298 ms、fingerprint `004b659f688e2f5a` | 96 chunks/半径32/256正規粒子条件ではない |
| B3 熱・反応 | セルの20%へenergyを設定。287 ms / 295 ms、fingerprint `7dd31ea01c323aec` | 96 chunksではない。反応・燃焼solverは未実装 |

測定値は2回のdebug runによるwall-clock経過時間で、性能保証やM6比較に使わない。現在のrunnerは固定worldの初期セルを埋め、B0〜B3の物理負荷を小さく再現する。96 chunks、entity/正規粒子、反応を含めない簡易proxyである。

再現可能な失敗sceneは`cargo run --locked --bin m0_baseline -- failure`。64×64初期worldで水を(10,10)に置き、次に半径65の砂ブラシを提出する。期待結果は`InvalidRadius`拒否かつ処理前後fingerprint不変、実測も一致 (`2114f4d5536a07b1`)。これは壊れたengine stateの既知不具合ではなく、拒否経路の再現fixtureである。

## M0出口チェック

- [x] toolchainと依存lockを確認
- [x] check/testと既存sampleを実行し、環境制約を記録
- [x] B0〜B3の小規模固定sceneと再実行fingerprint
- [x] 開発機のCPU/RAM/GPU/driver情報（CIMを使わないPowerShell経路で採取）
- [x] 再現可能な拒否sceneと期待不変条件を記録（engine bug sceneではない）
- [x] 設計ID一覧は[共通契約](02_CONTRACTS.md)、[検証項目](07_VALIDATION.md)、[ロードマップ](09_ROADMAP.md)に定義済み

M0出口条件を満たした。大規模B0〜B3、性能目標、allocation/memory観測はM6の測定対象として残す。

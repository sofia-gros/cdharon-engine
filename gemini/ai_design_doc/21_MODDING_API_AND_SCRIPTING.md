# スクリプティングエンジン (Rhai/WASM) & Mod フック機構 (`ai_design_doc/21_MODDING_API_AND_SCRIPTING.md`)

> **本書の目的**  
> Noita の Lua スクリプトのように、MOD 開発者が魔法のカスタム挙動やボスの攻撃ロジック、クエストイベントを記述できる軽量組み込みスクリプティング基盤（Rhai または WebAssembly）の統合仕様を定義します。

---

## 1. 組み込みスクリプトエンジンの選定（Rhai）

Rust ネイティブで動作し、ガベージコレクションがなく、C++ / C-ABI のバインディングが不要で極めて安全・高速な **Rhai（ライ）** を標準採用します。

* **選定理由**:
  * 依存関係が純粋な Rust クレートのみ。
  * サンドボックス化が容易（ファイルアクセスやシステムコールの禁止が可能）。
  * Rust の構造体や関数を型安全にスクリプトへ公開可能。

---

## 2. スクリプト API の公開とバインディング

```rust
use rhai::{Engine, Scope};

pub struct ScriptManager {
    engine: Engine,
}

impl ScriptManager {
    pub fn new() -> Self {
        let mut engine = Engine::new();

        // 1. 基本物理操作関数のバインド
        engine.register_fn("explode", |world: &mut World, x: i64, y: i64, radius: i64| {
            world.explode(x as i32, y as i32, radius as i32, 5.0);
        });

        engine.register_fn("spawn_particle", |world: &mut World, x: f64, y: f64, color_r: i64| {
            // パーティクル生成
        });

        Self { engine }
    }
}
```

### スクリプト側（魔法 MOD コード例: `spells/meteor.rhai`）
```rust
// 隕石魔法スクリプト
fn on_projectile_hit(world, hit_x, hit_y) {
    // 激しい大爆発
    explode(world, hit_x, hit_y, 25);
    
    // 周囲に溶岩を撒き散らす
    for i in 0..10 {
        spawn_falling_material(world, hit_x + rand_range(-10, 10), hit_y - 5, "Lava");
    }
}
```

---

## 3. セキュリティとサンドボックス化（Malware Prevention）

商用エンジンにおいて、サードパーティ MOD がユーザーの PC を破壊する（ランサムウェア・情報漏洩）リスクを完全に遮断します。
* ネットワーク通信（ソケット）、ローカルファイルへの無制限アクセス、OS コマンド実行 API をスクリプト環境から完全に隔離します。
* スクリプトの実行命令数（Fuel / ステップリミット）を設定し、無限ループによるゲームのフリーズを自動中断・エラー通知します。

# TS/JS & Go 開発者のための Rust サンドボックス物理入門 (`07_RUST_FOR_TS_GO_DEVS.md`)

> **本書の目的**  
> TypeScript / JavaScript や Go の開発経験があるプログラマーが、Rust の初学者として Falling Sand 物理エンジンを開発する際に直面する「メンタルモデルの違い」「所有権の壁」「メモリとビット演算の扱い方」を、親しみやすい比較を交えて明快に解説します。

---

## 1. 言語モデルの比較チートシート

| 概念 | TypeScript / JavaScript | Go | Rust (Charon Engine) |
|---|---|---|---|
| **メモリ管理** | V8 エンジンの GC (マーク＆スイープ) | ランタイム GC (並行三色マーキング) | **GC なし（コンパイル時に寿命決定）** |
| **データ構造** | オブジェクト `{ material: 1 }` (ヒープ参照) | 構造体 `struct` (値型 / ポインタ `*T`) | **固定サイズ構造体 `#[repr(C, align(4))]`** |
| **配列の連続性** | `Array` (内部でポインタ配列) / `Uint8Array` | スライス `[]T` (連続メモリの参照) | **固定長配列 `[Cell; 4096]` (純粋な連続メモリ)** |
| **状態の表現** | プロパティ `isFalling: true` | フィールド `IsFalling bool` | **ビットフラグ `flags & FLAG_FALLING != 0`** |
| **インターフェース** | `interface` / `type` (構造的部分型) | `interface` (ダックタイピング) | **`trait` (明示的実装 `impl Trait for T`)** |
| **エラー処理** | `try / catch` / `throw` | `if err != nil` | **`Result<T, E>` / `Option<T>` / `match` / `?`** |
| **変数の既定** | 再代入可能（`let`） | 再代入可能（`var`, `:=`） | **既定で不変（`mut` を付けない限り変更不可）** |

---

## 2. TS/Go 開発者が最もつまずく「Rust の 3 大壁」と攻略法

### ① 所有権と借用（Borrow Checker）の壁
#### TS/Go で書きがちなコード（Rust ではコンパイルエラー）
TS や Go では、配列をループで回しながら、同じ配列の別の要素を自由に書き換えることができます。
```typescript
// TS では普通に動くが、Rust では弾かれるパターン
for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
        const cell = world.cells[y * width + x]; // 読み取り参照
        if (cell.material === WATER) {
            world.cells[(y + 1) * width + x] = cell; // ❌ Rust: 配列全体の借用と書き換えが衝突！
        }
    }
}
```

#### なぜ Rust のコンパイラは怒るのか？
Rust には **「参照（`&T`）が生きている間は、誰にも可変参照（`&mut T`）を渡してはならない」** という絶対のルールがあるためです。イテレータで配列を借用しながら配列の中身を直接書き換えると、メモリの安全性が保証できないからです。

#### Charon Engine での解決パターン（インデックス駆動）
Rust でセルオートマトンを高速に回す定石は、**「イテレータを使わず、整数インデックス（`usize`）で値をコピー（Copy トレイト）して更新する」** ことです。
```rust
// Cell は 4 バイトであり、Copy トレイトを実装しているため「値の複製」が極めて高速
let curr_cell = self.cells[curr_idx]; // コピー（借用は発生しない！）

// 落下先が空きなら書き換え
if self.cells[down_idx].material == AIR {
    self.cells[curr_idx] = Cell::new(AIR);
    self.cells[down_idx] = curr_cell;
}
```
`Cell` に `#[derive(Clone, Copy)]` を付けておけば、ポインタの借用ではなく「4 バイトの数値の代入（Go の `int32` の代入と同じ）」になるため、借用チェッカーと戦う必要がなくなります。

---

### ② 「オブジェクト指向」から「データ指向 & ビット演算」への意識シフト
#### TS 脳で考えてしまうと……
```typescript
class Cell {
    material: Material;
    isFalling: boolean;
    isBurning: boolean;
    isWet: boolean;
    temperature: number;
}
```
これだと 1 セルあたり数十バイトを消費し、ガベージコレクションの負荷で画面がカクつきます。

#### Go の `iota` やビットマスクの感覚を思い出す
Go で定数フラグをビットシフトで書くのと全く同じ感覚で、Rust でも 1 バイト（`u8`）の中に 8 個のフラグを詰め込みます。

```rust
// ビットの定義（各ビットが独立したスイッチ）
pub const FLAG_CLOCK: u8   = 1 << 0; // 0b00000001 (1)
pub const FLAG_FALLING: u8 = 1 << 1; // 0b00000010 (2)
pub const FLAG_BURNING: u8 = 1 << 2; // 0b00000100 (4)

// 1. スイッチを ON にする (OR演算: |)
cell.flags |= FLAG_FALLING;

// 2. スイッチを OFF にする (AND-NOT演算: & !)
cell.flags &= !FLAG_FALLING;

// 3. スイッチが ON か調べる (AND演算: &)
let is_falling = (cell.flags & FLAG_FALLING) != 0;

// 4. スイッチを反転する (XOR演算: ^)
cell.flags ^= FLAG_CLOCK;
```
この 4 つのパターンだけ覚えておけば、ビット演算の 99% はマスターできます。

---

### ③ エラー処理：Go の `if err != nil` vs Rust の `Result` と `?`
Go では戻り値に `(Value, error)` を返しますが、Rust では `Result<T, E>` という Enum を使います。

```go
// Go スタイル
f, err := os.Open("materials.toml")
if err != nil {
    return err
}
```

```rust
// Rust スタイル（? 演算子で 1 行で書ける！）
let content = fs::read_to_string("materials.toml")?;
```
`?` 演算子は、成功なら値（`T`）を取り出し、失敗（`Err`）なら呼び出し元へ即座に `return` してくれる構文糖衣です。Go の `if err != nil { return err }` と完全に同等です。

---

## 3. 実在確認済み 日本語学習リファレンスリンク集

> 掲載されているすべてのリンクは、HTTP 通信により実在性を確認済みです（2026年10月確認）。

### 公式・定番の日本語入門ドキュメント
* [The Rust Programming Language 日本語版 (公式ドキュメント)](https://doc.rust-jp.rs/book-ja/)  
  *通称「The Book」。Rust 学習の決定版バイブル。*
  * [第4章: 所有権を理解する](https://doc.rust-jp.rs/book-ja/ch04-00-understanding-ownership.html) （TS/Go 開発者が最初に読むべき最重要章）
  * [第6章: Enumとパターンマッチング](https://doc.rust-jp.rs/book-ja/ch06-00-enums.html) （`Option` や `Result` の仕組み）
  * [第8章: コレクション (ベクタ Vec)](https://doc.rust-jp.rs/book-ja/ch08-01-vectors.html) （Go のスライスとの比較に最適）
  * [第10章: トレイト: 共通の振る舞いを定義する](https://doc.rust-jp.rs/book-ja/ch10-02-traits.html) （Go の interface との違い）
* [Tour of Rust 日本語版](https://tourofrust.com/00_ja.html)  
  *ブラウザ上でステップバイステップに Rust の基本機能を体験できる入門ツアー。*
* [Rust By Example 日本語版](https://doc.rust-jp.rs/rust-by-example-ja/)  
  *豊富なサンプルコードから文法を逆引きできる実践集。*
* [The Rustonomicon 日本語版 (Unsafe Rust の手引き)](https://doc.rust-jp.rs/rust-nomicon-ja/)  
  *低レイヤのメモリレイアウトやアライメントを深く学びたい場合の必読書。*

### サンドボックス物理・アルゴリズム（日本語解説）
* [セル・オートマトン - Wikipedia 日本語版](https://ja.wikipedia.org/wiki/%E3%82%BB%E3%83%AB%E3%83%BB%E3%82%AA%E3%83%BC%E3%83%88%E3%83%9E%E3%83%88%E3%83%B3)  
  *本エンジンの基礎理論であるセル・オートマトンの体系的解説。*
* [ビット演算 - Wikipedia 日本語版](https://ja.wikipedia.org/wiki/%E3%83%93%E3%83%83%E3%83%88%E6%BC%94%E7%AE%97)  
  *AND、OR、XOR、ビットシフトによるフラグ管理の数理的解説。*
* [ブレゼンハムのアルゴリズム - Wikipedia 日本語版](https://ja.wikipedia.org/wiki/%E3%83%96%E3%83%AC%E3%82%BC%E3%83%B3%E3%83%8F%E3%83%A0%E3%81%AE%E3%82%A2%E3%83%AB%E3%82%B4%E3%83%AA%E3%82%BA%E3%83%A0)  
  *高速粒子の弾道走査（すり抜け防止）に使用する整数直線アルゴリズム。*
* [連長圧縮 (Run-Length Encoding) - Wikipedia 日本語版](https://ja.wikipedia.org/wiki/%E9%80%A3%E9%95%B7%E5%9C%A7%E7%B8%AE)  
  *ワールドセーブデータの軽量バイナリ圧縮方式の解説。*

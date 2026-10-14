# 16-bit Material ID & O(1) 動的レジストリ完全仕様 (`ai_design_doc/19_MATERIAL_REGISTRY_16BIT.md`)

> **本書の目的**  
> 最大 65,535 種類（16-bit）のマテリアルを動的登録し、1 万個登録されても 1 ナノ秒で物性を引ける商用レジストリアーキテクチャの完全な内部仕様を定義します。

---

## 1. データ構造の完全仕様

```rust
pub struct MaterialRegistry {
    /// マテリアル ID（u16: 0 .. 65,535）をそのままインデックスとして参照
    materials: Vec<MaterialDef>,
    /// 名前から ID への逆引き O(1) インデックス
    name_to_id: HashMap<String, u16>,
    /// 頻繁に参照される物性（挙動、密度、流速）をキャッシュした平坦配列
    physics_cache: Vec<FastPhysicsProps>,
}

#[derive(Clone, Copy, Default)]
#[repr(C, align(8))]
pub struct FastPhysicsProps {
    pub behavior: Behavior,
    pub density: u8,
    pub max_fall_speed: u8,
    pub flow_speed: u8,
    pub can_be_washed_away: bool,
    pub padding: [u8; 3],
}
```

* **FastPhysicsProps キャッシュ**:
  物理ループ内でテクスチャデータ（64バイト）を触らず、物性パラメータ（8バイト）だけを連続した別の配列にキャッシュすることで、L1 キャッシュ汚染を最小限に防ぎます。

---

## 2. O(1) 参照の保証と安全性

```rust
impl MaterialRegistry {
    #[inline(always)]
    pub fn get(&self, id: u16) -> &MaterialDef {
        // 未登録 ID は AIR（0 番）に安全フォールバック（パニック防止）
        unsafe {
            if (id as usize) < self.materials.len() {
                self.materials.get_unchecked(id as usize)
            } else {
                self.materials.get_unchecked(0)
            }
        }
    }

    #[inline(always)]
    pub fn get_physics(&self, id: u16) -> FastPhysicsProps {
        unsafe {
            if (id as usize) < self.physics_cache.len() {
                *self.physics_cache.get_unchecked(id as usize)
            } else {
                *self.physics_cache.get_unchecked(0)
            }
        }
    }
}
```
* `get_unchecked` と境界フォールバックの組み合わせにより、商用ゲームに必要な**「ゼロオーバーヘッド」**と**「MOD 読み込み時の耐クラッシュ性」**を両立します。

# WGPU レンダリングパイプライン & フレームバッファ設計 (`ai_design_doc/14_WGPU_RENDERING_PIPELINE.md`)

> **本書の目的**  
> `wgpu`（WebGPU / Vulkan / DirectX 12 / Metal）をバックエンドとする商用レンダリングパイプラインのアーキテクチャを定義し、CPU ピクセルバッファから GPU VRAM への超高速転送とダブルバッファリングを確立します。

---

## 1. レンダリングアーキテクチャの概要

ピクセル物理ゲームにおいて、毎フレーム 20 万ピクセルを個別の Quad（四角形ポリゴン）として GPU にドローコールを発行するのは極めて非効率（数千ドローコールで GPU が窒息）です。  
Charon Engine は **「CPU/コンピュートシェーダーでの単一テクスチャ生成 $\to$ GPU 画面全体 1 ドローコール投影」** 方式を採用します。

```text
[CPU / Compute Shader]
論理フレームバッファ (640x360 RGBA8: 約 900 KiB)
        │
        ▼ (wgpu::Queue::write_texture / VRAM DMA転送: 0.1ms未満)
[GPU VRAM テクスチャ]
        │
        ▼ (頂点シェーダー: 全画面 フルスクリーンQuad)
        ▼ (フラグメントシェーダー: 整数スケーリング + CRT/ブルーム)
[画面ディスプレイ (1920x1080 / 4K)]
```

---

## 2. 整数スケーリング（Pixel-Perfect Integer Scaling）

低解像度（640x360）を現代の高解像度モニタ（1080p, 1440p, 4K）に表示する際、通常のバイリニア補間ではドット絵がぼやけます。  
シェーダー側で **厳密な整数倍率スケーリング + 端数レターボックス（黒帯）** を行い、常にクリスプなピクセルアートを保証します。

```wgsl
// WGSL フルスクリーン・フラグメントシェーダー例
@group(0) @binding(0) var screen_texture: texture_2d<f32>;
@group(0) @binding(1) var screen_sampler: sampler;

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    // 最近傍サンプリングでシャープなドット絵を維持
    return textureSample(screen_texture, screen_sampler, uv);
}
```

---

## 3. レンダリングレイヤー順序（Render Pass Order）

1. **Layer 0 (Background)**: 視差スクロール（Parallax）背景（山並み、地下の洞窟壁、空）。
2. **Layer 1 (World Grid)**: セル・オートマトンの全ピクセル（適応的 8x8 タイリング色）。
3. **Layer 2 (Fluid Highlights)**: 水面の波立ち、溶岩のグロー。
4. **Layer 3 (Particles)**: 自由飛翔パーティクル（火花、煙、血飛沫）。
5. **Layer 4 (Entities)**: プレイヤー、敵キャラクター、投射物。
6. **Layer 5 (Lighting Map)**: 2D 光線追跡ライティングマップ（乗算ブレンド）。
7. **Layer 6 (Foreground UI)**: ホットバー、HP/マナゲージ、カーソル。

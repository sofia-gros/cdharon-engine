//! Worldを書き換えずにspriteを抽出し、CPU参照描画または作者adapterへ渡す表示基盤。
//!
//! draw commandは画像のleaseを保持します。Scene終了・hot reloadの後も、提出済みframeが
//! 使う旧画像はframeの破棄まで生存します。GPUへの提出とその完了待ちはOS adapterの責務です。

use crate::{
    assets::{AssetHandle, AssetLease, AssetServer, AtlasAsset, ImageAsset, ImageRect},
    foundation::{EngineError, EngineResult, ErrorCode},
};

/// straight alphaのsRGB色。byte範囲で表し、NaNや範囲外の色を持ち込みません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub [u8; 4]);
impl Color {
    /// 不透明な黒。
    pub const BLACK: Self = Self([0, 0, 0, 255]);
    /// tintを変えない白。
    pub const WHITE: Self = Self([255, 255, 255, 255]);
    /// 完全透明。
    pub const TRANSPARENT: Self = Self([0, 0, 0, 0]);
}
/// 出力frame内のcamera矩形です。単位は論理pixelです。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    /// 左端。
    pub x: u32,
    /// 上端。
    pub y: u32,
    /// 幅。
    pub width: u32,
    /// 高さ。
    pub height: u32,
}
/// 描画とpointer逆変換で共通に使うcameraです。Worldの位置を変更しません。
#[derive(Clone, Copy, Debug)]
pub struct Camera2d {
    /// viewport中央が見るWorld位置。
    pub position: [f64; 2],
    /// 正の拡大率。
    pub zoom: f64,
    /// camera自体の回転radian。
    pub rotation: f64,
    /// 出力先の半開矩形。
    pub viewport: Viewport,
}
impl Camera2d {
    /// 左上を(0,0)とする通常のscreen cameraを作ります。
    pub fn screen(width: u32, height: u32) -> Self {
        Self {
            position: [f64::from(width) / 2.0, f64::from(height) / 2.0],
            zoom: 1.0,
            rotation: 0.0,
            viewport: Viewport {
                x: 0,
                y: 0,
                width,
                height,
            },
        }
    }
    /// cameraの値域を検証します。viewportがframe内にあるかは抽出時にも確認します。
    pub fn validate(&self) -> EngineResult<()> {
        if self.position.iter().any(|v| !v.is_finite())
            || !self.zoom.is_finite()
            || self.zoom <= 0.0
            || !self.rotation.is_finite()
            || self.viewport.width == 0
            || self.viewport.height == 0
            || self.viewport.x.checked_add(self.viewport.width).is_none()
            || self.viewport.y.checked_add(self.viewport.height).is_none()
        {
            return Err(invalid("camera値域"));
        }
        Ok(())
    }
    /// Worldの点をframeの論理pixelへ変換します。viewport外の点も表現できます。
    pub fn world_to_screen(&self, point: [f64; 2]) -> EngineResult<[f64; 2]> {
        self.validate()?;
        finite(point)?;
        let [x, y] = rotate(
            [point[0] - self.position[0], point[1] - self.position[1]],
            -self.rotation,
        );
        let result = [
            x * self.zoom + f64::from(self.viewport.x) + f64::from(self.viewport.width) / 2.0,
            y * self.zoom + f64::from(self.viewport.y) + f64::from(self.viewport.height) / 2.0,
        ];
        finite(result)?;
        Ok(result)
    }
    /// viewport外はNoneを返し、有効なWorld位置へ勝手に丸めません。
    pub fn screen_to_world(&self, point: [f64; 2]) -> EngineResult<Option<[f64; 2]>> {
        self.validate()?;
        finite(point)?;
        if point[0] < f64::from(self.viewport.x)
            || point[1] < f64::from(self.viewport.y)
            || point[0] >= f64::from(self.viewport.x + self.viewport.width)
            || point[1] >= f64::from(self.viewport.y + self.viewport.height)
        {
            return Ok(None);
        }
        Ok(Some(self.inverse(point)?))
    }
    fn inverse(&self, point: [f64; 2]) -> EngineResult<[f64; 2]> {
        let center = [
            f64::from(self.viewport.x) + f64::from(self.viewport.width) / 2.0,
            f64::from(self.viewport.y) + f64::from(self.viewport.height) / 2.0,
        ];
        let result = rotate(
            [
                (point[0] - center[0]) / self.zoom,
                (point[1] - center[1]) / self.zoom,
            ],
            self.rotation,
        );
        let result = [result[0] + self.position[0], result[1] + self.position[1]];
        finite(result)?;
        Ok(result)
    }
}
/// 最終frameをOS surfaceへ配置する共通変換です。余白をゲーム座標へ混ぜません。
#[derive(Clone, Copy, Debug)]
pub struct SurfaceMapping {
    scale: f64,
    offset: [f64; 2],
    logical: [u32; 2],
    surface: [u32; 2],
}
impl SurfaceMapping {
    /// letterboxを作ります。整数倍率でsurfaceが小さい場合は等倍の中央をclipします。
    pub fn fit(logical: [u32; 2], surface: [u32; 2], integer: bool) -> EngineResult<Self> {
        if logical.contains(&0) || surface.contains(&0) {
            return Err(invalid("surface寸法"));
        }
        let mut scale = (f64::from(surface[0]) / f64::from(logical[0]))
            .min(f64::from(surface[1]) / f64::from(logical[1]));
        if integer {
            scale = scale.floor().max(1.0);
        }
        Ok(Self {
            scale,
            offset: [
                (f64::from(surface[0]) - f64::from(logical[0]) * scale) / 2.0,
                (f64::from(surface[1]) - f64::from(logical[1]) * scale) / 2.0,
            ],
            logical,
            surface,
        })
    }
    /// OS pointerを論理frameへ変換し、letterbox部分ならNoneを返します。
    pub fn to_logical(&self, point: [f64; 2]) -> EngineResult<Option<[f64; 2]>> {
        finite(point)?;
        if point[0] < 0.0
            || point[1] < 0.0
            || point[0] >= f64::from(self.surface[0])
            || point[1] >= f64::from(self.surface[1])
        {
            return Ok(None);
        }
        let result = [
            (point[0] - self.offset[0]) / self.scale,
            (point[1] - self.offset[1]) / self.scale,
        ];
        Ok((result[0] >= 0.0
            && result[1] >= 0.0
            && result[0] < f64::from(self.logical[0])
            && result[1] < f64::from(self.logical[1]))
        .then_some(result))
    }
}
/// source画像をsampleする方式です。物理・入力の座標精度を変えません。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    /// pixel art向けの最近傍。
    Nearest,
    /// premultiplied linear色で補間します。
    Linear,
}
/// 一つのspriteの表示属性です。builderは値を集め、提出時にまとめて検証します。
#[derive(Clone, Copy, Debug)]
pub struct Sprite {
    /// anchorのWorld位置。
    pub position: [f64; 2],
    /// 表示寸法。Noneならsource frameのpixel寸法を使います。
    pub size: Option<[f64; 2]>,
    /// 回転の中心を0〜1の画像比率で指定します。範囲外anchorも意図的に使えます。
    pub anchor: [f64; 2],
    /// spriteの回転radian。
    pub rotation: f64,
    /// 左右・上下反転。
    pub flip: [bool; 2],
    /// 色と透明度の倍率。
    pub tint: Color,
    /// sheetのframe番号。
    pub frame: usize,
    /// 小さいlayerから描きます。
    pub layer: i32,
    /// 同layer内の奥行き。同値は提出順を保ちます。
    pub z: i32,
    /// textureのsample方法。
    pub filter: Filter,
}
impl Default for Sprite {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0],
            size: None,
            anchor: [0.0, 0.0],
            rotation: 0.0,
            flip: [false, false],
            tint: Color::WHITE,
            frame: 0,
            layer: 0,
            z: 0,
            filter: Filter::Nearest,
        }
    }
}
impl Sprite {
    /// 左上基準のspriteを作ります。
    pub fn new() -> Self {
        Self::default()
    }
    /// World位置を指定します。
    pub fn position(mut self, x: f64, y: f64) -> Self {
        self.position = [x, y];
        self
    }
    /// 表示寸法を指定します。sourceのpixel数や衝突形状は変更しません。
    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.size = Some([width, height]);
        self
    }
    /// source frameを選びます。
    pub fn frame(mut self, index: usize) -> Self {
        self.frame = index;
        self
    }
    /// layerとzを指定します。
    pub fn order(mut self, layer: i32, z: i32) -> Self {
        self.layer = layer;
        self.z = z;
        self
    }
    /// 中心anchorと回転を指定します。
    pub fn rotated(mut self, radians: f64) -> Self {
        self.anchor = [0.5, 0.5];
        self.rotation = radians;
        self
    }
    /// sample方法を指定します。
    pub fn filter(mut self, filter: Filter) -> Self {
        self.filter = filter;
        self
    }
}
/// 抽出済みdraw command。作者adapterも検証済みの同じ入力を受け取れます。
pub struct SpriteCommand {
    /// frame完了まで生存する画像。
    pub image: AssetLease<ImageAsset>,
    /// 検証済みsource範囲。
    pub source: ImageRect,
    /// spriteの表示属性。
    pub sprite: Sprite,
    /// このcommandが使うcameraのsnapshot。
    pub camera: Camera2d,
}
/// frameごとに作る、正規Worldへの可変参照を持たない表示snapshotです。
pub struct RenderFrame {
    /// 論理pixelの幅。
    pub width: u32,
    /// 論理pixelの高さ。
    pub height: u32,
    /// 各pixelの初期色。
    pub clear: Color,
    /// 提出順のcommand一覧。
    pub sprites: Vec<SpriteCommand>,
    limit: usize,
    failure: Option<EngineError>,
}
impl RenderFrame {
    /// 上限付きの抽出先を作ります。巨大なframeは描画adapterも拒否します。
    pub fn new(width: u32, height: u32, limit: usize) -> EngineResult<Self> {
        frame_bytes(width, height)?;
        if width == 0 || height == 0 || limit == 0 {
            return Err(invalid("描画frame寸法または上限"));
        }
        Ok(Self {
            width,
            height,
            clear: Color::BLACK,
            sprites: Vec::new(),
            limit,
            failure: None,
        })
    }
    /// 次frameへ移ります。旧frameのleaseを解放します。
    pub fn reset(&mut self) {
        self.sprites.clear();
        self.clear = Color::BLACK;
        self.failure = None;
    }
    /// 明示したcameraで描画要求を提出します。
    pub fn draw<'a>(&'a mut self, assets: &'a AssetServer, camera: Camera2d) -> DrawContext<'a> {
        DrawContext {
            frame: self,
            assets,
            camera,
        }
    }
    /// Sceneが提出時エラーを無視した場合も、不完全なframeを表示しません。
    pub fn validate(&self) -> EngineResult<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        frame_bytes(self.width, self.height)?;
        if self.width == 0 || self.height == 0 || self.sprites.len() > self.limit {
            return Err(invalid("描画frameの寸法または容量"));
        }
        for command in &self.sprites {
            command
                .source
                .validate(command.image.width, command.image.height)?;
            command.camera.validate()?;
            let viewport = command.camera.viewport;
            if viewport.x + viewport.width > self.width
                || viewport.y + viewport.height > self.height
            {
                return Err(invalid("描画viewport範囲"));
            }
            finite(command.sprite.position)?;
            finite(command.sprite.anchor)?;
            if !command.sprite.rotation.is_finite() {
                return Err(invalid("sprite回転"));
            }
            if let Some(size) = command.sprite.size {
                finite(size)?;
                if size[0] <= 0.0 || size[1] <= 0.0 {
                    return Err(invalid("sprite表示寸法"));
                }
            }
        }
        Ok(())
    }
}
/// 作者が描画adapterを置換するための入口です。WorldやOS event loopを所有しません。
pub trait RenderAdapter {
    /// 検証済みsnapshotをRGBA8 targetへ描画します。失敗したtargetを表示してはいけません。
    fn render(&mut self, frame: &RenderFrame, target: &mut [u8]) -> EngineResult<()>;
}
/// Sceneへ一時的に渡す描画提出の権限です。
pub struct DrawContext<'a> {
    frame: &'a mut RenderFrame,
    assets: &'a AssetServer,
    camera: Camera2d,
}
impl DrawContext<'_> {
    /// このframeの背景色を指定します。並存Sceneの後の明示指定が優先されます。
    pub fn clear(&mut self, color: Color) {
        self.frame.clear = color;
    }
    /// 画像・sheetを描画します。画像の生存期間はcommandが保持します。
    pub fn image(&mut self, image: AssetHandle<ImageAsset>, sprite: Sprite) -> EngineResult<()> {
        let result = (|| {
            let image = self.assets.lease(image)?;
            let source = *image
                .frames
                .get(sprite.frame)
                .ok_or_else(|| invalid("sprite frame番号"))?;
            self.submit(image, source, sprite)
        })();
        self.remember(result)
    }
    /// 名前付きatlas frameを描画します。
    pub fn atlas(
        &mut self,
        atlas: AssetHandle<AtlasAsset>,
        name: &str,
        sprite: Sprite,
    ) -> EngineResult<()> {
        let result = (|| {
            let atlas = self.assets.lease(atlas)?;
            let source = *atlas
                .frames
                .get(name)
                .ok_or_else(|| invalid("atlas frame名"))?;
            self.submit(atlas.image.clone(), source, sprite)
        })();
        self.remember(result)
    }
    fn remember(&mut self, result: EngineResult<()>) -> EngineResult<()> {
        if let Err(error) = &result {
            self.frame.failure.get_or_insert_with(|| error.clone());
        }
        result
    }
    fn submit(
        &mut self,
        image: AssetLease<ImageAsset>,
        source: ImageRect,
        sprite: Sprite,
    ) -> EngineResult<()> {
        self.camera.validate()?;
        if self.camera.viewport.x + self.camera.viewport.width > self.frame.width
            || self.camera.viewport.y + self.camera.viewport.height > self.frame.height
        {
            return Err(invalid("camera viewportがframe外"));
        }
        finite(sprite.position)?;
        finite(sprite.anchor)?;
        if !sprite.rotation.is_finite() {
            return Err(invalid("sprite回転"));
        }
        if let Some(size) = sprite.size {
            finite(size)?;
            if size[0] <= 0.0 || size[1] <= 0.0 {
                return Err(invalid("sprite寸法"));
            }
        }
        if self.frame.sprites.len() >= self.frame.limit {
            return Err(EngineError::capacity(
                "frame sprite数",
                self.frame.sprites.len() as u64 + 1,
                self.frame.limit as u64,
            ));
        }
        self.frame
            .sprites
            .try_reserve(1)
            .map_err(crate::foundation::error::allocation)?;
        self.frame.sprites.push(SpriteCommand {
            image,
            source,
            sprite,
            camera: self.camera,
        });
        Ok(())
    }
}
/// 最適化経路と比較できるscalar rendererです。sample数の上限で極端な負荷を明示拒否します。
pub struct ScalarRenderer {
    /// 一frameのpixel sample仕事量の上限。
    pub max_samples: u64,
}
impl Default for ScalarRenderer {
    fn default() -> Self {
        Self {
            max_samples: 100_000_000,
        }
    }
}
impl RenderAdapter for ScalarRenderer {
    fn render(&mut self, frame: &RenderFrame, target: &mut [u8]) -> EngineResult<()> {
        frame.validate()?;
        let needed = frame_bytes(frame.width, frame.height)?;
        if needed != target.len() {
            return Err(invalid("描画target byte数"));
        }
        let mut order = Vec::new();
        order
            .try_reserve_exact(frame.sprites.len())
            .map_err(crate::foundation::error::allocation)?;
        order.extend(0..frame.sprites.len());
        order.sort_by_key(|index| {
            let sprite = &frame.sprites[*index].sprite;
            (sprite.layer, sprite.z, *index)
        });
        // 全commandの範囲と費用を先に確認し、上限拒否でtargetを途中まで変更しません。
        let mut bounds = Vec::new();
        bounds
            .try_reserve_exact(frame.sprites.len())
            .map_err(crate::foundation::error::allocation)?;
        let mut work = 0_u64;
        for index in &order {
            let command = &frame.sprites[*index];
            let bound = command_bounds(command)?;
            let samples = u64::from(bound[2] - bound[0]) * u64::from(bound[3] - bound[1]);
            work = work
                .checked_add(samples)
                .ok_or_else(|| invalid("sample数overflow"))?;
            if work > self.max_samples {
                return Err(EngineError::capacity(
                    "frame sample数",
                    work,
                    self.max_samples,
                ));
            }
            bounds.push(bound);
        }
        for pixel in target.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&frame.clear.0);
        }
        for (index, bound) in order.into_iter().zip(bounds) {
            let command = &frame.sprites[index];
            let sprite = command.sprite;
            let size = sprite.size.unwrap_or([
                f64::from(command.source.width),
                f64::from(command.source.height),
            ]);
            for y in bound[1]..bound[3] {
                for x in bound[0]..bound[2] {
                    let point = command
                        .camera
                        .inverse([f64::from(x) + 0.5, f64::from(y) + 0.5])?;
                    let local = rotate(
                        [point[0] - sprite.position[0], point[1] - sprite.position[1]],
                        -sprite.rotation,
                    );
                    let mut uv = [
                        local[0] / size[0] + sprite.anchor[0],
                        local[1] / size[1] + sprite.anchor[1],
                    ];
                    if uv[0] < 0.0 || uv[1] < 0.0 || uv[0] >= 1.0 || uv[1] >= 1.0 {
                        continue;
                    }
                    for (axis, value) in uv.iter_mut().enumerate() {
                        if sprite.flip[axis] {
                            *value = 1.0 - *value;
                        }
                    }
                    let mut source = sample(&command.image, command.source, uv, sprite.filter);
                    let tint = linear(sprite.tint.0);
                    for channel in 0..3 {
                        source[channel] *= tint[channel];
                    }
                    source[3] *= tint[3];
                    let offset =
                        (u64::from(y) * u64::from(frame.width) + u64::from(x)) as usize * 4;
                    let destination = linear(
                        target[offset..offset + 4]
                            .try_into()
                            .map_err(|_| invalid("target pixel"))?,
                    );
                    let alpha = source[3] + destination[3] * (1.0 - source[3]);
                    let mut color = [0.0; 4];
                    color[3] = alpha;
                    if alpha > 0.0 {
                        for channel in 0..3 {
                            color[channel] = (source[channel] * source[3]
                                + destination[channel] * destination[3] * (1.0 - source[3]))
                                / alpha;
                        }
                    }
                    target[offset..offset + 4].copy_from_slice(&srgb(color));
                }
            }
        }
        Ok(())
    }
}
fn command_bounds(command: &SpriteCommand) -> EngineResult<[u32; 4]> {
    let sprite = command.sprite;
    let size = sprite.size.unwrap_or([
        f64::from(command.source.width),
        f64::from(command.source.height),
    ]);
    let mut minimum = [f64::INFINITY; 2];
    let mut maximum = [f64::NEG_INFINITY; 2];
    for corner in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
        let point = rotate(
            [
                (corner[0] - sprite.anchor[0]) * size[0],
                (corner[1] - sprite.anchor[1]) * size[1],
            ],
            sprite.rotation,
        );
        let screen = command
            .camera
            .world_to_screen([point[0] + sprite.position[0], point[1] + sprite.position[1]])?;
        for axis in 0..2 {
            minimum[axis] = minimum[axis].min(screen[axis]);
            maximum[axis] = maximum[axis].max(screen[axis]);
        }
    }
    let view = command.camera.viewport;
    Ok([
        minimum[0]
            .floor()
            .clamp(f64::from(view.x), f64::from(view.x + view.width)) as u32,
        minimum[1]
            .floor()
            .clamp(f64::from(view.y), f64::from(view.y + view.height)) as u32,
        maximum[0]
            .ceil()
            .clamp(f64::from(view.x), f64::from(view.x + view.width)) as u32,
        maximum[1]
            .ceil()
            .clamp(f64::from(view.y), f64::from(view.y + view.height)) as u32,
    ])
}
fn sample(image: &ImageAsset, rect: ImageRect, uv: [f64; 2], filter: Filter) -> [f64; 4] {
    let fetch = |x: i64, y: i64| {
        let x = x.clamp(0, i64::from(rect.width) - 1) as u32 + rect.x;
        let y = y.clamp(0, i64::from(rect.height) - 1) as u32 + rect.y;
        let offset = (u64::from(y) * u64::from(image.width) + u64::from(x)) as usize * 4;
        linear(
            image.rgba[offset..offset + 4]
                .try_into()
                .expect("検証済みRGBA8画像"),
        )
    };
    if filter == Filter::Nearest {
        return fetch(
            (uv[0] * f64::from(rect.width)).floor() as i64,
            (uv[1] * f64::from(rect.height)).floor() as i64,
        );
    }
    let x = uv[0] * f64::from(rect.width) - 0.5;
    let y = uv[1] * f64::from(rect.height) - 0.5;
    let fx = x - x.floor();
    let fy = y - y.floor();
    let mut result = [0.0; 4];
    for (dx, dy, weight) in [
        (0, 0, (1.0 - fx) * (1.0 - fy)),
        (1, 0, fx * (1.0 - fy)),
        (0, 1, (1.0 - fx) * fy),
        (1, 1, fx * fy),
    ] {
        let value = fetch(x.floor() as i64 + dx, y.floor() as i64 + dy);
        for channel in 0..3 {
            result[channel] += value[channel] * value[3] * weight;
        }
        result[3] += value[3] * weight;
    }
    if result[3] > 0.0 {
        for channel in 0..3 {
            result[channel] /= result[3];
        }
    }
    result
}
fn linear(color: [u8; 4]) -> [f64; 4] {
    let mut result = [0.0; 4];
    for channel in 0..3 {
        let v = f64::from(color[channel]) / 255.0;
        result[channel] = if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        };
    }
    result[3] = f64::from(color[3]) / 255.0;
    result
}
fn srgb(color: [f64; 4]) -> [u8; 4] {
    let mut result = [0; 4];
    for channel in 0..3 {
        let v = color[channel].clamp(0.0, 1.0);
        result[channel] = ((if v <= 0.0031308 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }) * 255.0)
            .round() as u8;
    }
    result[3] = (color[3].clamp(0.0, 1.0) * 255.0).round() as u8;
    result
}
fn rotate([x, y]: [f64; 2], angle: f64) -> [f64; 2] {
    let (sin, cos) = angle.sin_cos();
    [x * cos - y * sin, x * sin + y * cos]
}
fn finite(point: [f64; 2]) -> EngineResult<()> {
    if point.iter().any(|value| !value.is_finite()) {
        Err(invalid("表示座標の有限性"))
    } else {
        Ok(())
    }
}
fn invalid(message: &'static str) -> EngineError {
    EngineError::new(ErrorCode::InvalidValue, message)
}

// u32の積自体がu64へ収まっても、RGBAの4倍でoverflowするため、各演算を検査します。
fn frame_bytes(width: u32, height: u32) -> EngineResult<usize> {
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|bytes| bytes.checked_mul(4))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .ok_or_else(|| invalid("描画RGBA byte数のoverflow"))
}

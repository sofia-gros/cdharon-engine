//! sourceを検証済みのCPU資産へ変換します。GPUや音声deviceへ依存しません。
//!
//! decode前に寸法・source量、decode中に展開量を制限します。画像は8bit straight alphaの
//! sRGBへ統一し、音はinterleaved f32 PCMへ統一します。元データのhashは別途manifestへ記録します。

use super::vfs::capacity;
use crate::foundation::{EngineError, EngineResult, ErrorCode};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Cursor};

/// importerが使用する、外部ファイルから引き上げられない展開上限です。
#[derive(Clone, Copy, Debug)]
pub struct ImportLimits {
    /// 圧縮されたsourceの上限byte数。
    pub source_bytes: usize,
    /// 一資産の展開後の上限byte数。
    pub decoded_bytes: usize,
    /// 画像の各軸の最大pixel数。
    pub image_dimension: u32,
    /// sheetやatlasへ登録できるframe数。
    pub frames: usize,
}
impl Default for ImportLimits {
    fn default() -> Self {
        Self {
            source_bytes: 64 * 1024 * 1024,
            decoded_bytes: 128 * 1024 * 1024,
            image_dimension: 16384,
            frames: 65536,
        }
    }
}
/// cacheへ保持する作者型の契約です。正規のWorld状態を資産へ隠さないでください。
pub trait Asset: Send + Sync + 'static {
    /// cache予算へ計上する保持byte数。依存資産の保持量は二重計上しません。
    fn resident_bytes(&self) -> usize;
    /// cacheへの公開前に作者型の不変条件を検査します。標準型は内部長さも照合します。
    fn validate(&self, limits: ImportLimits) -> EngineResult<()> {
        if self.resident_bytes() > limits.decoded_bytes {
            return Err(capacity(
                "資産byte数",
                self.resident_bytes(),
                limits.decoded_bytes,
            ));
        }
        Ok(())
    }
}
/// 拡張形式も標準形式と同じsource/hash/cache/scope経路へ接続します。
pub trait Importer<T: Asset>: Send + Sync + 'static {
    /// アルゴリズムと設定を識別する安定文字列。設定変更時はこの値も変えます。
    fn identity(&self) -> String;
    /// このimportが参照する検証済み依存のcontent hash。独自atlas等も追跡できます。
    fn dependencies(&self) -> Vec<[u8; 32]> {
        Vec::new()
    }
    /// 外部データを検証してCPU資産を作ります。失敗時は旧世代を変更しません。
    fn import(&self, source: &[u8], limits: ImportLimits) -> EngineResult<T>;
}
/// 画像内の半開矩形。単位はsource画像のpixelです。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageRect {
    /// 左端。
    pub x: u32,
    /// 上端。
    pub y: u32,
    /// 幅。
    pub width: u32,
    /// 高さ。
    pub height: u32,
}
impl ImageRect {
    /// ゼロ寸法・overflow・画像外のframeを拒否します。
    pub fn validate(self, width: u32, height: u32) -> EngineResult<Self> {
        if self.width == 0
            || self.height == 0
            || self.x.checked_add(self.width).is_none_or(|end| end > width)
            || self
                .y
                .checked_add(self.height)
                .is_none_or(|end| end > height)
        {
            return Err(invalid("画像frame範囲"));
        }
        Ok(self)
    }
}
/// GPUの有無に依存しないRGBA画像です。正規の衝突maskとは別の表示資産です。
pub struct ImageAsset {
    /// 幅pixel。
    pub width: u32,
    /// 高さpixel。
    pub height: u32,
    /// straight alpha、sRGB、row-majorのRGBA8。
    pub rgba: Vec<u8>,
    /// sheetのframe。通常画像は画像全体の一つです。
    pub frames: Vec<ImageRect>,
}
impl Asset for ImageAsset {
    fn resident_bytes(&self) -> usize {
        self.rgba.capacity().saturating_add(
            self.frames
                .capacity()
                .saturating_mul(std::mem::size_of::<ImageRect>()),
        )
    }
    fn validate(&self, limits: ImportLimits) -> EngineResult<()> {
        if self.rgba.len() != image_bytes(self.width, self.height, limits)?
            || self.frames.is_empty()
            || self.frames.len() > limits.frames
            || self.resident_bytes() > limits.decoded_bytes
        {
            return Err(invalid("RGBAまたはframe配列長"));
        }
        for frame in &self.frames {
            frame.validate(self.width, self.height)?;
        }
        Ok(())
    }
}
/// PNGを通常画像または等間隔sheetとして読み込むimporterです。
#[derive(Clone, Copy, Default)]
pub struct PngImporter {
    /// Noneなら通常画像。Someなら各frameの幅と高さを指定します。
    pub frame_size: Option<(u32, u32)>,
}
impl Importer<ImageAsset> for PngImporter {
    fn identity(&self) -> String {
        format!("charon:png-v1-srgb-straight8:{:?}", self.frame_size)
    }
    fn import(&self, source: &[u8], limits: ImportLimits) -> EngineResult<ImageAsset> {
        source_limit(source, limits)?;
        // IHDRの寸法を確認してからdecoderへ渡し、展開前の確保量を限定します。
        if source.len() < 33 || &source[..8] != b"\x89PNG\r\n\x1a\n" || &source[12..16] != b"IHDR" {
            return Err(invalid("PNG header"));
        }
        let width = u32::from_be_bytes(source[16..20].try_into().map_err(|_| invalid("PNG幅"))?);
        let height = u32::from_be_bytes(source[20..24].try_into().map_err(|_| invalid("PNG高さ"))?);
        let length = image_bytes(width, height, limits)?;
        let mut decoder = png::Decoder::new_with_limits(
            Cursor::new(source),
            png::Limits {
                bytes: limits.decoded_bytes,
            },
        );
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().map_err(decode_error)?;
        if reader.info().animation_control.is_some() {
            return Err(EngineError::new(
                ErrorCode::UnsupportedCapability,
                "APNGはtimeline importerで扱います",
            ));
        }
        // 色空間が不明なprofileをsRGBと偽らず、専用importerへ明示的に委ねます。
        if reader.info().icc_profile.is_some() {
            return Err(EngineError::new(
                ErrorCode::UnsupportedCapability,
                "ICC PNGには色変換importerが必要",
            ));
        }
        let gamma = if reader.info().srgb.is_none() {
            reader
                .info()
                .gamma()
                .map(|value| f64::from(value.into_value()))
        } else {
            None
        };
        if gamma.is_some_and(|value| value <= 0.0 || !value.is_finite()) {
            return Err(invalid("PNG gamma値域"));
        }
        if reader.info().srgb.is_none()
            && reader.info().chromaticities().is_some_and(|value| {
                value
                    != png::SourceChromaticities::new(
                        (0.3127, 0.3290),
                        (0.64, 0.33),
                        (0.30, 0.60),
                        (0.15, 0.06),
                    )
            })
        {
            return Err(EngineError::new(
                ErrorCode::UnsupportedCapability,
                "非sRGB原色には色変換importerが必要",
            ));
        }
        let output = reader
            .output_buffer_size()
            .ok_or_else(|| invalid("PNG展開byte数"))?;
        if output > length {
            return Err(invalid("PNG出力寸法"));
        }
        let mut decoded = Vec::new();
        decoded
            .try_reserve_exact(output)
            .map_err(crate::foundation::error::allocation)?;
        decoded.resize(output, 0);
        let info = reader.next_frame(&mut decoded).map_err(decode_error)?;
        reader.finish().map_err(decode_error)?;
        if info.width != width || info.height != height {
            return Err(invalid("PNG寸法の不一致"));
        }
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(length)
            .map_err(crate::foundation::error::allocation)?;
        match info.color_type {
            png::ColorType::Rgba => rgba.extend_from_slice(&decoded[..info.buffer_size()]),
            png::ColorType::Rgb => {
                for pixel in decoded[..info.buffer_size()].as_chunks::<3>().0 {
                    rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
                }
            }
            png::ColorType::Grayscale => {
                for value in &decoded[..info.buffer_size()] {
                    rgba.extend_from_slice(&[*value, *value, *value, 255]);
                }
            }
            png::ColorType::GrayscaleAlpha => {
                for pixel in decoded[..info.buffer_size()].as_chunks::<2>().0 {
                    rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
                }
            }
            _ => return Err(invalid("PNG色形式")),
        }
        if rgba.len() != length {
            return Err(invalid("PNG pixel数"));
        }
        if let Some(gamma) = gamma {
            let mut lookup = [0_u8; 256];
            for (index, value) in lookup.iter_mut().enumerate() {
                let linear = (index as f64 / 255.0).powf(1.0 / gamma);
                let srgb = if linear <= 0.0031308 {
                    linear * 12.92
                } else {
                    1.055 * linear.powf(1.0 / 2.4) - 0.055
                };
                *value = (srgb * 255.0).round().clamp(0.0, 255.0) as u8;
            }
            for pixel in rgba.as_chunks_mut::<4>().0 {
                for value in &mut pixel[..3] {
                    *value = lookup[usize::from(*value)];
                }
            }
        }
        let (fw, fh) = self.frame_size.unwrap_or((width, height));
        if fw == 0 || fh == 0 || width % fw != 0 || height % fh != 0 {
            return Err(invalid("sheet frame寸法"));
        }
        let count = u64::from(width / fw) * u64::from(height / fh);
        if count > limits.frames as u64 {
            return Err(capacity("sheet frame数", count as usize, limits.frames));
        }
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(count as usize)
            .map_err(crate::foundation::error::allocation)?;
        for y in 0..height / fh {
            for x in 0..width / fw {
                frames.push(ImageRect {
                    x: x * fw,
                    y: y * fh,
                    width: fw,
                    height: fh,
                });
            }
        }
        Ok(ImageAsset {
            width,
            height,
            rgba,
            frames,
        })
    }
}
pub(crate) fn image_bytes(width: u32, height: u32, limits: ImportLimits) -> EngineResult<usize> {
    if width == 0
        || height == 0
        || width > limits.image_dimension
        || height > limits.image_dimension
    {
        return Err(invalid("画像寸法"));
    }
    let length = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|bytes| bytes.checked_mul(4))
        .ok_or_else(|| invalid("画像RGBA byte数のoverflow"))?;
    if length > limits.decoded_bytes as u64 {
        return Err(EngineError::capacity(
            "画像RGBA byte数",
            length,
            limits.decoded_bytes as u64,
        ));
    }
    Ok(length as usize)
}

/// 名前付きatlasのsource schemaです。回転・trimを黙って無視する形式にはしません。
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtlasDefinition {
    /// schemaの版。現在は1です。
    pub schema_version: u16,
    /// 名前からsource矩形への対応。
    #[serde(deserialize_with = "unique_frames")]
    pub frames: BTreeMap<String, ImageRect>,
}
fn unique_frames<'de, D: serde::Deserializer<'de>>(
    input: D,
) -> Result<BTreeMap<String, ImageRect>, D::Error> {
    struct Frames;
    impl<'de> serde::de::Visitor<'de> for Frames {
        type Value = BTreeMap<String, ImageRect>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("一意なframe名と矩形の対応")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut input: M,
        ) -> Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, value)) = input.next_entry::<String, ImageRect>()? {
                if result.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("atlas frame名が重複しています"));
                }
            }
            Ok(result)
        }
    }
    input.deserialize_map(Frames)
}
/// JSON atlasの矩形を保持します。画像との接続と依存hashはloaderが検証します。
pub struct AtlasAsset {
    /// atlasが参照する画像の保持権。旧atlasが生存中は旧画像も生存します。
    pub image: super::AssetLease<ImageAsset>,
    /// 検証済みのframe一覧。
    pub frames: BTreeMap<String, ImageRect>,
}
impl Asset for AtlasAsset {
    fn resident_bytes(&self) -> usize {
        self.frames
            .keys()
            .map(|key| key.len() + std::mem::size_of::<ImageRect>() + 64)
            .sum()
    }
}
/// WAVとOgg Vorbisから得たinterleaved PCM。再生deviceはE09のaudio adapterが担当します。
pub struct AudioAsset {
    /// 一秒あたりのsample frame数。
    pub sample_rate: u32,
    /// channel数。mono/stereo等の配置を隠しません。
    pub channels: u16,
    /// -1から1の有限f32、channel交互のsampleです。
    pub samples: Vec<f32>,
}
impl Asset for AudioAsset {
    fn resident_bytes(&self) -> usize {
        self.samples.capacity() * 4
    }
}
/// headerでWAV/Oggを判別します。拡張子だけで形式を決めません。
pub struct AudioImporter;
impl Importer<AudioAsset> for AudioImporter {
    fn identity(&self) -> String {
        "charon:pcm-v1".into()
    }
    fn import(&self, source: &[u8], limits: ImportLimits) -> EngineResult<AudioAsset> {
        source_limit(source, limits)?;
        let mut samples = Vec::new();
        let (sample_rate, channels) = if source.starts_with(b"RIFF") {
            let mut reader = hound::WavReader::new(Cursor::new(source)).map_err(decode_error)?;
            let spec = reader.spec();
            if spec.channels == 0
                || spec.sample_rate == 0
                || spec.bits_per_sample == 0
                || spec.bits_per_sample > 32
            {
                return Err(invalid("WAV sample形式"));
            }
            let count = reader.len() as usize;
            if count > limits.decoded_bytes / 4 {
                return Err(capacity("PCM sample数", count, limits.decoded_bytes / 4));
            }
            samples
                .try_reserve_exact(count)
                .map_err(crate::foundation::error::allocation)?;
            match spec.sample_format {
                hound::SampleFormat::Float => {
                    for value in reader.samples::<f32>() {
                        samples.push(value.map_err(decode_error)?);
                    }
                }
                hound::SampleFormat::Int => {
                    let scale = 2_f64.powi(i32::from(spec.bits_per_sample) - 1);
                    for value in reader.samples::<i32>() {
                        samples.push((f64::from(value.map_err(decode_error)?) / scale) as f32);
                    }
                }
            }
            (spec.sample_rate, spec.channels)
        } else if source.starts_with(b"OggS") {
            let mut reader = lewton::inside_ogg::OggStreamReader::new(Cursor::new(source))
                .map_err(decode_error)?;
            let format = (
                reader.ident_hdr.audio_sample_rate,
                u16::from(reader.ident_hdr.audio_channels),
            );
            while let Some(packet) = reader.read_dec_packet_itl().map_err(decode_error)? {
                if format
                    != (
                        reader.ident_hdr.audio_sample_rate,
                        u16::from(reader.ident_hdr.audio_channels),
                    )
                {
                    return Err(invalid("連結Oggの形式変更"));
                }
                if packet.len() > (limits.decoded_bytes / 4).saturating_sub(samples.len()) {
                    return Err(capacity(
                        "PCM sample数",
                        samples.len() + packet.len(),
                        limits.decoded_bytes / 4,
                    ));
                }
                samples
                    .try_reserve_exact(packet.len())
                    .map_err(crate::foundation::error::allocation)?;
                samples.extend(packet.into_iter().map(|value| f32::from(value) / 32768.0));
            }
            format
        } else {
            return Err(invalid("音声header"));
        };
        if sample_rate == 0
            || channels == 0
            || samples.len() % usize::from(channels) != 0
            || samples
                .iter()
                .any(|value| !value.is_finite() || !(-1.0..=1.0).contains(value))
        {
            return Err(invalid("PCM値域またはchannel数"));
        }
        Ok(AudioAsset {
            sample_rate,
            channels,
            samples,
        })
    }
}
/// 元fontを保持します。Faceは必要時に借用して作り、自己参照構造やunsafeを使いません。
pub struct FontAsset {
    /// 検証済みTTF/OTFのsourceです。
    pub bytes: Vec<u8>,
    /// font collection内のface番号。
    pub face_index: u32,
}
impl FontAsset {
    /// fontデータから借用したFace。戻り値はこの資産より長く保持できません。
    pub fn face(&self) -> EngineResult<ttf_parser::Face<'_>> {
        ttf_parser::Face::parse(&self.bytes, self.face_index).map_err(decode_error)
    }
}
impl Asset for FontAsset {
    fn resident_bytes(&self) -> usize {
        self.bytes.capacity()
    }
}
/// TTF/OTF/collectionの選択faceを検証します。文字組みはE09の独立サービスです。
pub struct FontImporter {
    /// 通常fontは0。collectionは対象faceを指定します。
    pub face_index: u32,
}
impl Importer<FontAsset> for FontImporter {
    fn identity(&self) -> String {
        format!("charon:font-v1:{}", self.face_index)
    }
    fn import(&self, source: &[u8], limits: ImportLimits) -> EngineResult<FontAsset> {
        source_limit(source, limits)?;
        if source.len() > limits.decoded_bytes {
            return Err(capacity("font byte数", source.len(), limits.decoded_bytes));
        }
        let face = ttf_parser::Face::parse(source, self.face_index).map_err(decode_error)?;
        if face.number_of_glyphs() == 0 {
            return Err(invalid("空font"));
        }
        Ok(FontAsset {
            bytes: source.to_vec(),
            face_index: self.face_index,
        })
    }
}
pub(crate) fn source_limit(bytes: &[u8], limits: ImportLimits) -> EngineResult<()> {
    if bytes.len() > limits.source_bytes {
        return Err(capacity(
            "import source byte数",
            bytes.len(),
            limits.source_bytes,
        ));
    }
    Ok(())
}
pub(crate) fn invalid(message: &'static str) -> EngineError {
    EngineError::new(ErrorCode::InvalidEncoding, message)
}
pub(crate) fn decode_error(cause: impl std::fmt::Display) -> EngineError {
    invalid("資産import").detail(cause.to_string())
}

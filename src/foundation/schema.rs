//! 型付きcodecの登録、payload検証、明示的な旧版からの移行。
//!
//! TypeIdは実行時の取り違え検知だけに使い、保存形式へ書き込みません。
//! 移行は番号が増える有限列に限定し、未知の新しい版を旧版として読ませません。

use super::codec::{Decoder, Encoder, StateCodec};
use super::error::{EngineError, EngineResult, ErrorCode, allocation};
use super::key::StableKey;
use std::any::TypeId;

/// 一つの旧版から上位版へ変換する、作者登録の処理です。
pub type Migration = fn(&[u8], &mut Encoder) -> EngineResult<()>;

#[derive(Clone, Copy)]
struct Step {
    from: u32,
    to: u32,
    input_limit: usize,
    migrate: Migration,
}

/// 一つの正規状態型の定義です。fieldの意味と値域はcodecで検証します。
pub struct Schema {
    key: StableKey,
    version: u32,
    max_bytes: usize,
    type_id: TypeId,
    validate: fn(&[u8]) -> EngineResult<()>,
    migrations: Vec<Step>,
}

impl Schema {
    /// 作者型を登録します。版0は未指定と区別するため拒否します。
    pub fn of<T: StateCodec + 'static>(
        key: StableKey,
        version: u32,
        max_bytes: usize,
    ) -> EngineResult<Self> {
        if version == 0 || max_bytes > u32::MAX as usize {
            return Err(mismatch());
        }
        Ok(Self {
            key,
            version,
            max_bytes,
            type_id: TypeId::of::<T>(),
            validate: validate::<T>,
            migrations: Vec::new(),
        })
    }

    /// 安定キーを返します。
    pub fn key(&self) -> StableKey {
        self.key
    }
    /// 現在の保存版を返します。
    pub fn version(&self) -> u32 {
        self.version
    }
    /// その型の最大payload byte数を返します。
    pub fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// 型の一致を調べます。永続的な型同定にはkey/versionを使用します。
    pub fn is<T: 'static>(&self) -> bool {
        self.type_id == TypeId::of::<T>()
    }

    /// 現行版の値域、長さ、余分な末尾を検証します。
    pub fn validate(&self, bytes: &[u8]) -> EngineResult<()> {
        if bytes.len() > self.max_bytes {
            return Err(EngineError::capacity(
                "schema payload byte数",
                bytes.len() as u64,
                self.max_bytes as u64,
            ));
        }
        (self.validate)(bytes)
    }

    /// 旧版の移行を追加します。入力側にも独立したbyte上限を持たせます。
    pub fn add_migration(
        &mut self,
        from: u32,
        to: u32,
        input_limit: usize,
        migrate: Migration,
    ) -> EngineResult<()> {
        if from == 0 || from >= to || to > self.version || input_limit > u32::MAX as usize {
            return Err(mismatch());
        }
        if self.migrations.iter().any(|step| step.from == from) {
            return Err(EngineError::new(
                ErrorCode::DuplicateKey,
                "schemaの移行元版",
            ));
        }
        self.migrations.try_reserve(1).map_err(allocation)?;
        self.migrations.push(Step {
            from,
            to,
            input_limit,
            migrate,
        });
        Ok(())
    }

    /// 旧payloadを候補bufferへ移行し、最終的な現行schemaで検証します。
    ///
    /// 元のbyte列や登録済みstoreは変更しません。途中版も出力上限を適用します。
    pub fn upgrade(&self, mut version: u32, bytes: &[u8]) -> EngineResult<Vec<u8>> {
        if version == 0 || version > self.version {
            return Err(mismatch());
        }
        let first_limit = if version == self.version {
            self.max_bytes
        } else {
            self.migrations
                .iter()
                .find(|step| step.from == version)
                .ok_or_else(mismatch)?
                .input_limit
        };
        if bytes.len() > first_limit {
            return Err(EngineError::capacity(
                "移行入力byte数",
                bytes.len() as u64,
                first_limit as u64,
            ));
        }
        let mut current = copy_bytes(bytes)?;
        while version != self.version {
            let step = self
                .migrations
                .iter()
                .find(|step| step.from == version)
                .ok_or_else(mismatch)?;
            if current.len() > step.input_limit {
                return Err(mismatch());
            }
            let output_limit = if step.to == self.version {
                self.max_bytes
            } else {
                self.migrations
                    .iter()
                    .find(|next| next.from == step.to)
                    .ok_or_else(mismatch)?
                    .input_limit
            };
            let mut output = Encoder::new(output_limit);
            (step.migrate)(&current, &mut output)?;
            current = output.finish();
            version = step.to;
        }
        self.validate(&current)?;
        Ok(current)
    }

    pub(crate) fn try_copy(&self) -> EngineResult<Self> {
        let mut migrations = Vec::new();
        migrations
            .try_reserve_exact(self.migrations.len())
            .map_err(allocation)?;
        migrations.extend_from_slice(&self.migrations);
        Ok(Self {
            key: self.key,
            version: self.version,
            max_bytes: self.max_bytes,
            type_id: self.type_id,
            validate: self.validate,
            migrations,
        })
    }
}

/// 登録順に関係なく、安定キー順でschemaを解決するregistryです。
#[derive(Default)]
pub struct SchemaRegistry {
    schemas: Vec<Schema>,
}

impl SchemaRegistry {
    /// 重複を拒否し、キー順へ挿入します。失敗時は既存定義を維持します。
    pub fn register(&mut self, schema: Schema) -> EngineResult<()> {
        let index = match self
            .schemas
            .binary_search_by_key(&schema.key, |item| item.key)
        {
            Ok(_) => {
                return Err(EngineError::new(
                    ErrorCode::DuplicateKey,
                    "schemaの安定キー",
                ));
            }
            Err(index) => index,
        };
        self.schemas.try_reserve(1).map_err(allocation)?;
        self.schemas.insert(index, schema);
        Ok(())
    }

    /// 存在するschemaだけを返します。欠損を空型として扱いません。
    pub fn get(&self, key: StableKey) -> EngineResult<&Schema> {
        let index = self
            .schemas
            .binary_search_by_key(&key, |item| item.key)
            .map_err(|_| EngineError::new(ErrorCode::MissingKey, "schemaの安定キー"))?;
        Ok(&self.schemas[index])
    }

    /// 安定キー順に列挙します。
    pub fn iter(&self) -> impl Iterator<Item = &Schema> {
        self.schemas.iter()
    }

    pub(crate) fn try_copy(&self) -> EngineResult<Self> {
        let mut schemas = Vec::new();
        schemas
            .try_reserve_exact(self.schemas.len())
            .map_err(allocation)?;
        for schema in &self.schemas {
            schemas.push(schema.try_copy()?);
        }
        Ok(Self { schemas })
    }
}

fn validate<T: StateCodec>(bytes: &[u8]) -> EngineResult<()> {
    let mut input = Decoder::new(bytes, bytes.len())?;
    T::decode(&mut input)?;
    input.finish()
}

pub(crate) fn copy_bytes(bytes: &[u8]) -> EngineResult<Vec<u8>> {
    let mut output = Vec::new();
    output.try_reserve_exact(bytes.len()).map_err(allocation)?;
    output.extend_from_slice(bytes);
    Ok(output)
}
pub(crate) fn mismatch() -> EngineError {
    EngineError::new(ErrorCode::SchemaMismatch, "状態schemaの型または版")
}

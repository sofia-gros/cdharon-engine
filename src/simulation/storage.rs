//! 停止tick時点のsnapshotを世代番号付きで保存し、破損世代から復旧する機能を提供します。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::entities::player::Player;
use crate::game::session::{GameError, GameSession};
use crate::simulation::determinism::Fnv64;
use crate::simulation::material::MaterialRegistry;
use crate::simulation::world::{SnapshotError, World};

const MAGIC: &[u8; 8] = b"CHRSAVE1";
const HEADER_BYTES: usize = 8 + 8 + 8 + 8;
pub const MAX_GENERATION_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageError {
    Io(String),
    TooLarge,
    Corrupt,
    NoValidGeneration,
    GenerationOverflow,
    InvalidSnapshot(SnapshotError),
    InvalidBundle(GameError),
}
impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "snapshot storage error: {self:?}")
    }
}
impl std::error::Error for StorageError {}

#[derive(Debug)]
pub struct LoadedGeneration {
    pub generation: u64,
    pub payload: Vec<u8>,
    pub recovered_older_generation: bool,
}

pub struct LoadedWorldGeneration {
    pub generation: u64,
    pub world: World,
    pub recovered_older_generation: bool,
}

pub struct LoadedBundleGeneration {
    pub generation: u64,
    pub world: World,
    pub game: GameSession,
    pub player: Player,
    pub recovered_older_generation: bool,
}

#[derive(Clone, Debug)]
pub struct GenerationStore {
    root: PathBuf,
    max_bytes: usize,
}

impl GenerationStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            max_bytes: MAX_GENERATION_BYTES,
        }
    }

    pub fn with_max_bytes(
        root: impl Into<PathBuf>,
        max_bytes: usize,
    ) -> Result<Self, StorageError> {
        if max_bytes == 0 || max_bytes > MAX_GENERATION_BYTES {
            return Err(StorageError::TooLarge);
        }
        Ok(Self {
            root: root.into(),
            max_bytes,
        })
    }

    /// 完全な世代fileを書き込み同期した後、同一directory内のrenameで読取可能世代として公開します。
    /// 途中失敗では現在の有効世代を置換せず、再起動時に部分fileを最新世代と誤認させません。
    pub fn save(&self, payload: &[u8]) -> Result<u64, StorageError> {
        self.save_injected(payload, None)
    }

    fn save_injected(
        &self,
        payload: &[u8],
        fault: Option<SaveFaultPoint>,
    ) -> Result<u64, StorageError> {
        if payload.len() > self.max_bytes {
            return Err(StorageError::TooLarge);
        }
        fs::create_dir_all(&self.root).map_err(io_error)?;
        let generation = self.next_generation()?;
        let final_path = self.generation_path(generation);
        let temp_path = self.root.join(format!(".generation-{generation}.tmp"));
        let result = (|| {
            if fault == Some(SaveFaultPoint::CreateTemp) {
                return Err(injected_io_error());
            }
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp_path)
                .map_err(io_error)?;
            file.write_all(MAGIC).map_err(io_error)?;
            file.write_all(&generation.to_le_bytes())
                .map_err(io_error)?;
            file.write_all(&(payload.len() as u64).to_le_bytes())
                .map_err(io_error)?;
            file.write_all(&checksum(payload).to_le_bytes())
                .map_err(io_error)?;
            if fault == Some(SaveFaultPoint::PartialWrite) {
                return Err(injected_io_error());
            }
            file.write_all(payload).map_err(io_error)?;
            if fault == Some(SaveFaultPoint::AfterWrite) {
                return Err(injected_io_error());
            }
            if fault == Some(SaveFaultPoint::Flush) {
                return Err(injected_io_error());
            }
            file.sync_all().map_err(io_error)?;
            drop(file);
            if fault == Some(SaveFaultPoint::Rename) {
                return Err(injected_io_error());
            }
            fs::rename(&temp_path, &final_path).map_err(io_error)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result?;
        self.retain_latest(generation, 3);
        Ok(generation)
    }

    pub fn save_world(&self, world: &World) -> Result<u64, StorageError> {
        let snapshot = world
            .encode_snapshot()
            .map_err(StorageError::InvalidSnapshot)?;
        self.save(&snapshot)
    }

    pub fn save_bundle(
        &self,
        world: &World,
        game: &GameSession,
        player: &Player,
    ) -> Result<u64, StorageError> {
        let bundle = game
            .encode_bundle(world, player)
            .map_err(StorageError::InvalidBundle)?;
        self.save(&bundle)
    }

    pub fn load_world(
        &self,
        registry: MaterialRegistry,
    ) -> Result<LoadedWorldGeneration, StorageError> {
        let saved = self.load_latest()?;
        let world = World::decode_snapshot(&saved.payload, registry)
            .map_err(StorageError::InvalidSnapshot)?;
        Ok(LoadedWorldGeneration {
            generation: saved.generation,
            world,
            recovered_older_generation: saved.recovered_older_generation,
        })
    }

    pub fn load_bundle(
        &self,
        registry: MaterialRegistry,
    ) -> Result<LoadedBundleGeneration, StorageError> {
        let saved = self.load_latest()?;
        let (world, game, player) = GameSession::decode_bundle(&saved.payload, registry)
            .map_err(StorageError::InvalidBundle)?;
        Ok(LoadedBundleGeneration {
            generation: saved.generation,
            world,
            game,
            player,
            recovered_older_generation: saved.recovered_older_generation,
        })
    }

    /// 新しい順に完全性を検証し、最初に読めた世代を返します。
    /// より新しい破損fileを飛ばした場合はその事実も返し、古い有効世代へ黙って戻ったことを隠しません。
    pub fn load_latest(&self) -> Result<LoadedGeneration, StorageError> {
        let mut generations = self.list_generations()?;
        generations.sort_unstable_by(|a, b| b.cmp(a));
        let mut skipped_invalid = false;
        for generation in generations {
            match self.read_generation(generation) {
                Ok(payload) => {
                    return Ok(LoadedGeneration {
                        generation,
                        payload,
                        recovered_older_generation: skipped_invalid,
                    });
                }
                Err(_) => skipped_invalid = true,
            }
        }
        Err(StorageError::NoValidGeneration)
    }

    fn next_generation(&self) -> Result<u64, StorageError> {
        let mut latest = 0;
        for generation in self.list_generations()? {
            latest = latest.max(generation);
        }
        latest
            .checked_add(1)
            .ok_or(StorageError::GenerationOverflow)
    }

    fn generation_path(&self, generation: u64) -> PathBuf {
        self.root.join(format!("generation-{generation:020}.save"))
    }

    fn list_generations(&self) -> Result<Vec<u64>, StorageError> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let entries = fs::read_dir(&self.root).map_err(io_error)?;
        let mut values = Vec::new();
        for entry in entries {
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(number) = name
                .strip_prefix("generation-")
                .and_then(|name| name.strip_suffix(".save"))
                && let Ok(generation) = number.parse::<u64>()
            {
                values.push(generation);
            }
        }
        Ok(values)
    }

    fn read_generation(&self, generation: u64) -> Result<Vec<u8>, StorageError> {
        let path = self.generation_path(generation);
        let metadata = fs::metadata(&path).map_err(io_error)?;
        let size = usize::try_from(metadata.len()).map_err(|_| StorageError::TooLarge)?;
        if size < HEADER_BYTES
            || size
                > self
                    .max_bytes
                    .checked_add(HEADER_BYTES)
                    .ok_or(StorageError::TooLarge)?
        {
            return Err(StorageError::TooLarge);
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| StorageError::TooLarge)?;
        File::open(path)
            .map_err(io_error)?
            .take((self.max_bytes + HEADER_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() != size || &bytes[..8] != MAGIC {
            return Err(StorageError::Corrupt);
        }
        let stored_generation =
            u64::from_le_bytes(bytes[8..16].try_into().map_err(|_| StorageError::Corrupt)?);
        let payload_len = u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| StorageError::Corrupt)?,
        );
        let expected_checksum = u64::from_le_bytes(
            bytes[24..32]
                .try_into()
                .map_err(|_| StorageError::Corrupt)?,
        );
        let payload_len = usize::try_from(payload_len).map_err(|_| StorageError::Corrupt)?;
        if stored_generation != generation || payload_len != size - HEADER_BYTES {
            return Err(StorageError::Corrupt);
        }
        let payload = bytes.split_off(HEADER_BYTES);
        if checksum(&payload) != expected_checksum {
            return Err(StorageError::Corrupt);
        }
        Ok(payload)
    }

    fn retain_latest(&self, newest: u64, count: usize) {
        let Ok(mut generations) = self.list_generations() else {
            return;
        };
        generations.sort_unstable_by(|a, b| b.cmp(a));
        for stale in generations.into_iter().skip(count) {
            if stale != newest {
                let _ = fs::remove_file(self.generation_path(stale));
            }
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SaveFaultPoint {
    CreateTemp,
    PartialWrite,
    AfterWrite,
    Flush,
    Rename,
}

fn injected_io_error() -> StorageError {
    io_error(std::io::Error::other("injected save failure"))
}

fn checksum(payload: &[u8]) -> u64 {
    let mut hash = Fnv64::default();
    hash.write(b"charon-save-checksum-v1");
    hash.write(payload);
    hash.finish()
}
fn io_error(error: std::io::Error) -> StorageError {
    StorageError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "charon-generation-test-{}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn generations_roundtrip_retain_previous_and_fallback_after_corruption() {
        let root = temp_root();
        let store = GenerationStore::new(&root);
        assert_eq!(store.save(b"generation one").unwrap(), 1);
        assert_eq!(store.save(b"generation two").unwrap(), 2);
        assert_eq!(store.save(b"generation three").unwrap(), 3);
        assert_eq!(store.load_latest().unwrap().payload, b"generation three");
        let newest = store.generation_path(3);
        let mut bytes = fs::read(&newest).unwrap();
        *bytes.last_mut().unwrap() ^= 0x80;
        fs::write(&newest, bytes).unwrap();
        let loaded = store.load_latest().unwrap();
        assert_eq!(loaded.generation, 2);
        assert_eq!(loaded.payload, b"generation two");
        assert!(loaded.recovered_older_generation);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejected_oversized_save_preserves_latest_generation() {
        let root = temp_root();
        let store = GenerationStore::with_max_bytes(&root, 4).unwrap();
        store.save(b"good").unwrap();
        assert_eq!(store.save(b"too large"), Err(StorageError::TooLarge));
        assert_eq!(store.load_latest().unwrap().payload, b"good");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn world_snapshot_generation_restores_candidate_state() {
        let root = temp_root();
        let store = GenerationStore::new(&root);
        let mut world = World::new(8, 8).unwrap();
        world.update().unwrap();
        store.save_world(&world).unwrap();
        let loaded = store.load_world(MaterialRegistry::with_defaults()).unwrap();
        assert_eq!(loaded.world.state_fingerprint(), world.state_fingerprint());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bundle_generation_restores_world_game_and_player_together() {
        use crate::core::fixed::Fixed;
        let root = temp_root();
        let store = GenerationStore::new(&root);
        let world = World::new(8, 8).unwrap();
        let game = GameSession::new(4, crate::simulation::material::STONE, "sample:stone").unwrap();
        let player = Player::new(
            Fixed::from_integer(2).unwrap(),
            Fixed::from_integer(3).unwrap(),
        );
        store.save_bundle(&world, &game, &player).unwrap();
        let loaded = store
            .load_bundle(MaterialRegistry::with_defaults())
            .unwrap();
        assert_eq!(loaded.world.state_fingerprint(), world.state_fingerprint());
        assert_eq!(loaded.game, game);
        assert_eq!(loaded.player, player);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_temp_creation_leaves_previous_generation_readable() {
        let root = temp_root();
        let store = GenerationStore::new(&root);
        store.save(b"committed").unwrap();
        fs::write(root.join(".generation-2.tmp"), b"occupied temp name").unwrap();
        assert!(matches!(store.save(b"candidate"), Err(StorageError::Io(_))));
        assert_eq!(store.load_latest().unwrap().payload, b"committed");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn injected_write_flush_and_publish_failures_keep_last_valid_generation() {
        let root = temp_root();
        let store = GenerationStore::new(&root);
        store.save(b"committed generation").unwrap();
        for point in [
            SaveFaultPoint::CreateTemp,
            SaveFaultPoint::PartialWrite,
            SaveFaultPoint::AfterWrite,
            SaveFaultPoint::Flush,
            SaveFaultPoint::Rename,
        ] {
            assert!(matches!(
                store.save_injected(b"candidate generation", Some(point)),
                Err(StorageError::Io(_))
            ));
            let loaded = store.load_latest().unwrap();
            assert_eq!(loaded.generation, 1);
            assert_eq!(loaded.payload, b"committed generation");
            assert!(!root.join(".generation-2.tmp").exists());
        }
        let _ = fs::remove_dir_all(root);
    }
}

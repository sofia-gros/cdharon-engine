//! 64×64 cell storage with checked signed world-coordinate addressing.

use std::collections::HashMap;
use std::ops::{Index, IndexMut};

use crate::simulation::cell::Cell;

pub const CHUNK_EDGE: i32 = 64;
pub const CHUNK_AREA: usize = 64 * 64;
pub const WORLD_COORD_MIN: i32 = -1_048_576;
pub const WORLD_COORD_MAX: i32 = 1_048_575;
/// Initial resident allocation limit. Creator policy can raise this in a later API version.
pub const INITIAL_MAX_CHUNKS: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChunkCoord {
    pub x: i32,
    pub y: i32,
}

impl ChunkCoord {
    pub fn from_cell(x: i32, y: i32) -> Self {
        Self {
            x: x.div_euclid(CHUNK_EDGE),
            y: y.div_euclid(CHUNK_EDGE),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkStoreError {
    InvalidDimensions,
    CoordinateOverflow,
    CapacityExceeded,
    AllocationFailed,
}

/// A finite rectangular region backed by 64×64 chunks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkedCells {
    origin_x: i32,
    origin_y: i32,
    width: u32,
    height: u32,
    first_chunk_x: i32,
    first_chunk_y: i32,
    chunks_x: usize,
    chunks_y: usize,
    chunks: Vec<Box<[Cell]>>,
    remote_chunks: HashMap<ChunkCoord, Box<[Cell]>>,
}

impl ChunkedCells {
    pub fn try_new(
        origin_x: i32,
        origin_y: i32,
        width: u32,
        height: u32,
        fill: Cell,
    ) -> Result<Self, ChunkStoreError> {
        if width == 0 || height == 0 || width > 65_536 || height > 65_536 {
            return Err(ChunkStoreError::InvalidDimensions);
        }
        let end_x = i64::from(origin_x) + i64::from(width) - 1;
        let end_y = i64::from(origin_y) + i64::from(height) - 1;
        if origin_x < WORLD_COORD_MIN
            || origin_y < WORLD_COORD_MIN
            || end_x > i64::from(WORLD_COORD_MAX)
            || end_y > i64::from(WORLD_COORD_MAX)
        {
            return Err(ChunkStoreError::CoordinateOverflow);
        }

        let first_chunk_x = origin_x.div_euclid(CHUNK_EDGE);
        let first_chunk_y = origin_y.div_euclid(CHUNK_EDGE);
        let last_chunk_x = (end_x as i32).div_euclid(CHUNK_EDGE);
        let last_chunk_y = (end_y as i32).div_euclid(CHUNK_EDGE);
        let chunks_x = (i64::from(last_chunk_x) - i64::from(first_chunk_x) + 1) as usize;
        let chunks_y = (i64::from(last_chunk_y) - i64::from(first_chunk_y) + 1) as usize;
        let chunk_count = chunks_x
            .checked_mul(chunks_y)
            .ok_or(ChunkStoreError::CapacityExceeded)?;
        if chunk_count > INITIAL_MAX_CHUNKS {
            return Err(ChunkStoreError::CapacityExceeded);
        }

        let mut chunks = Vec::new();
        chunks
            .try_reserve_exact(chunk_count)
            .map_err(|_| ChunkStoreError::AllocationFailed)?;
        for _ in 0..chunk_count {
            let mut cells = Vec::new();
            cells
                .try_reserve_exact(CHUNK_AREA)
                .map_err(|_| ChunkStoreError::AllocationFailed)?;
            cells.resize(CHUNK_AREA, fill);
            chunks.push(cells.into_boxed_slice());
        }

        Ok(Self {
            origin_x,
            origin_y,
            width,
            height,
            first_chunk_x,
            first_chunk_y,
            chunks_x,
            chunks_y,
            chunks,
            remote_chunks: HashMap::new(),
        })
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.width as usize * self.height as usize
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        false
    }

    #[inline]
    pub fn chunk_count(&self) -> usize {
        self.chunks.len() + self.remote_chunks.len()
    }

    /// Atomically reserve additional signed-coordinate chunks outside the initial rectangle.
    pub fn ensure_chunks(
        &mut self,
        coordinates: &[ChunkCoord],
        resident_limit: usize,
        fill: Cell,
    ) -> Result<usize, ChunkStoreError> {
        let mut requested = coordinates.to_vec();
        requested.sort_unstable();
        requested.dedup();
        if requested.iter().any(|coordinate| {
            let min_x = i64::from(coordinate.x) * i64::from(CHUNK_EDGE);
            let min_y = i64::from(coordinate.y) * i64::from(CHUNK_EDGE);
            min_x < i64::from(WORLD_COORD_MIN)
                || min_y < i64::from(WORLD_COORD_MIN)
                || min_x > i64::from(WORLD_COORD_MAX)
                || min_y > i64::from(WORLD_COORD_MAX)
        }) {
            return Err(ChunkStoreError::CoordinateOverflow);
        }
        requested.retain(|coord| {
            !self.contains_world_cell(
                coord.x.saturating_mul(CHUNK_EDGE),
                coord.y.saturating_mul(CHUNK_EDGE),
            ) && !self.remote_chunks.contains_key(coord)
        });

        let final_count = self
            .chunk_count()
            .checked_add(requested.len())
            .ok_or(ChunkStoreError::CapacityExceeded)?;
        if final_count > resident_limit || final_count > INITIAL_MAX_CHUNKS {
            return Err(ChunkStoreError::CapacityExceeded);
        }

        self.remote_chunks
            .try_reserve(requested.len())
            .map_err(|_| ChunkStoreError::AllocationFailed)?;
        let mut prepared = Vec::new();
        prepared
            .try_reserve_exact(requested.len())
            .map_err(|_| ChunkStoreError::AllocationFailed)?;
        for coord in &requested {
            let mut cells = Vec::new();
            cells
                .try_reserve_exact(CHUNK_AREA)
                .map_err(|_| ChunkStoreError::AllocationFailed)?;
            cells.resize(CHUNK_AREA, fill);
            prepared.push((*coord, cells.into_boxed_slice()));
        }
        for (coord, cells) in prepared {
            self.remote_chunks.insert(coord, cells);
        }
        Ok(requested.len())
    }

    /// Remove a dynamic chunk. Initial finite-region chunks cannot be evicted by this method.
    pub fn remove_chunk(&mut self, coordinate: ChunkCoord) -> bool {
        if self.chunk_index(coordinate.x, coordinate.y).is_some() {
            return false;
        }
        self.remote_chunks.remove(&coordinate).is_some()
    }

    #[inline]
    pub fn get(&self, index: usize) -> Option<&Cell> {
        if index >= self.len() {
            return None;
        }
        let x = self.origin_x + (index % self.width as usize) as i32;
        let y = self.origin_y + (index / self.width as usize) as i32;
        self.get_world(x, y)
    }

    #[inline]
    pub fn get_world(&self, x: i32, y: i32) -> Option<&Cell> {
        if !self.contains_world_cell(x, y) {
            let coordinate = ChunkCoord::from_cell(x, y);
            let chunk = self.remote_chunks.get(&coordinate)?;
            let local_x = x.rem_euclid(CHUNK_EDGE) as usize;
            let local_y = y.rem_euclid(CHUNK_EDGE) as usize;
            return chunk.get(local_y * CHUNK_EDGE as usize + local_x);
        }
        let chunk_x = x.div_euclid(CHUNK_EDGE);
        let chunk_y = y.div_euclid(CHUNK_EDGE);
        let chunk_index = self.chunk_index(chunk_x, chunk_y)?;
        let local_x = x.rem_euclid(CHUNK_EDGE) as usize;
        let local_y = y.rem_euclid(CHUNK_EDGE) as usize;
        self.chunks[chunk_index].get(local_y * CHUNK_EDGE as usize + local_x)
    }

    #[inline]
    pub fn get_world_mut(&mut self, x: i32, y: i32) -> Option<&mut Cell> {
        if !self.contains_world_cell(x, y) {
            let coordinate = ChunkCoord::from_cell(x, y);
            let chunk = self.remote_chunks.get_mut(&coordinate)?;
            let local_x = x.rem_euclid(CHUNK_EDGE) as usize;
            let local_y = y.rem_euclid(CHUNK_EDGE) as usize;
            return chunk.get_mut(local_y * CHUNK_EDGE as usize + local_x);
        }
        let chunk_x = x.div_euclid(CHUNK_EDGE);
        let chunk_y = y.div_euclid(CHUNK_EDGE);
        let chunk_index = self.chunk_index(chunk_x, chunk_y)?;
        let local_x = x.rem_euclid(CHUNK_EDGE) as usize;
        let local_y = y.rem_euclid(CHUNK_EDGE) as usize;
        self.chunks[chunk_index].get_mut(local_y * CHUNK_EDGE as usize + local_x)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Cell> {
        (0..self.len()).map(|index| &self[index])
    }

    pub fn remote_chunks(&self) -> impl Iterator<Item = (ChunkCoord, &[Cell])> {
        self.remote_chunks
            .iter()
            .map(|(coordinate, cells)| (*coordinate, cells.as_ref()))
    }

    pub fn set_remote_cell(&mut self, x: i32, y: i32, cell: Cell) -> bool {
        let Some(target) = self.get_world_mut(x, y) else {
            return false;
        };
        *target = cell;
        true
    }

    pub fn initial_region(&self) -> (i32, i32, u32, u32) {
        (self.origin_x, self.origin_y, self.width, self.height)
    }

    pub fn is_initial_chunk(&self, coordinate: ChunkCoord) -> bool {
        self.chunk_index(coordinate.x, coordinate.y).is_some()
    }

    pub fn swap(&mut self, left: usize, right: usize) {
        assert!(
            left < self.len() && right < self.len(),
            "cell index outside finite region"
        );
        if left == right {
            return;
        }
        let left_cell = self[left];
        self[left] = self[right];
        self[right] = left_cell;
    }

    fn chunk_index(&self, chunk_x: i32, chunk_y: i32) -> Option<usize> {
        let local_x = i64::from(chunk_x) - i64::from(self.first_chunk_x);
        let local_y = i64::from(chunk_y) - i64::from(self.first_chunk_y);
        if local_x < 0
            || local_y < 0
            || local_x >= self.chunks_x as i64
            || local_y >= self.chunks_y as i64
        {
            return None;
        }
        Some(local_y as usize * self.chunks_x + local_x as usize)
    }

    #[inline]
    fn contains_world_cell(&self, x: i32, y: i32) -> bool {
        let x = i64::from(x);
        let y = i64::from(y);
        x >= i64::from(self.origin_x)
            && y >= i64::from(self.origin_y)
            && x < i64::from(self.origin_x) + i64::from(self.width)
            && y < i64::from(self.origin_y) + i64::from(self.height)
    }
}

impl<'a> IntoIterator for &'a ChunkedCells {
    type Item = &'a Cell;
    type IntoIter = Box<dyn Iterator<Item = &'a Cell> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

impl Index<usize> for ChunkedCells {
    type Output = Cell;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).expect("cell index outside finite region")
    }
}

impl IndexMut<usize> for ChunkedCells {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        assert!(index < self.len(), "cell index outside finite region");
        let x = self.origin_x + (index % self.width as usize) as i32;
        let y = self.origin_y + (index / self.width as usize) as i32;
        self.get_world_mut(x, y)
            .expect("cell index outside finite region")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_world_coordinates_use_euclidean_chunk_mapping() {
        assert_eq!(ChunkCoord::from_cell(-1, -64), ChunkCoord { x: -1, y: -1 });
        assert_eq!(ChunkCoord::from_cell(-64, -65), ChunkCoord { x: -1, y: -2 });
        assert_eq!(ChunkCoord::from_cell(-65, 0), ChunkCoord { x: -2, y: 0 });
    }

    #[test]
    fn rectangular_region_maps_cells_across_negative_chunk_boundary() {
        let mut cells = ChunkedCells::try_new(-65, -1, 130, 2, Cell::new(0)).unwrap();
        *cells.get_world_mut(-65, -1).unwrap() = Cell::new(10);
        *cells.get_world_mut(-1, 0).unwrap() = Cell::new(20);
        *cells.get_world_mut(64, 0).unwrap() = Cell::new(30);

        assert_eq!(cells.get(0).unwrap().material, 10);
        assert_eq!(cells.get(194).unwrap().material, 20);
        assert_eq!(cells.get(259).unwrap().material, 30);
        assert_eq!(cells.chunk_count(), 8);
        assert!(cells.get_world(-66, -1).is_none());
        assert!(cells.get_world(65, 0).is_none());
    }

    #[test]
    fn rejects_regions_over_resident_chunk_limit_before_allocation() {
        assert_eq!(
            ChunkedCells::try_new(0, 0, 65_536, 65_536, Cell::new(0)).unwrap_err(),
            ChunkStoreError::CapacityExceeded
        );
    }

    #[test]
    fn remote_chunks_are_allocated_atomically_at_negative_coordinates() {
        let mut cells = ChunkedCells::try_new(0, 0, 64, 64, Cell::new(0)).unwrap();
        let coords = [ChunkCoord { x: -2, y: 3 }, ChunkCoord { x: 4, y: -5 }];
        assert_eq!(cells.ensure_chunks(&coords, 3, Cell::new(7)), Ok(2));
        assert_eq!(cells.get_world(-65, 192).unwrap().material, 7);
        assert_eq!(cells.get_world(256, -320).unwrap().material, 7);
        assert!(cells.remove_chunk(coords[0]));
        assert!(cells.get_world(-65, 192).is_none());
    }

    #[test]
    fn remote_chunk_capacity_failure_does_not_publish_partial_chunks() {
        let mut cells = ChunkedCells::try_new(0, 0, 64, 64, Cell::new(0)).unwrap();
        let coords = [ChunkCoord { x: 2, y: 0 }, ChunkCoord { x: 3, y: 0 }];
        assert_eq!(
            cells.ensure_chunks(&coords, 2, Cell::new(9)),
            Err(ChunkStoreError::CapacityExceeded)
        );
        assert_eq!(cells.chunk_count(), 1);
        assert!(cells.get_world(128, 0).is_none());
    }

    #[test]
    fn world_coordinate_contract_is_enforced_for_initial_and_remote_chunks() {
        assert_eq!(
            ChunkedCells::try_new(WORLD_COORD_MAX, 0, 2, 1, Cell::new(0)),
            Err(ChunkStoreError::CoordinateOverflow)
        );
        let mut cells = ChunkedCells::try_new(0, 0, 64, 64, Cell::new(0)).unwrap();
        assert_eq!(
            cells.ensure_chunks(&[ChunkCoord { x: 16_384, y: 0 }], 10, Cell::new(0)),
            Err(ChunkStoreError::CoordinateOverflow)
        );
    }
}

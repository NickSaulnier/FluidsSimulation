pub mod mac;

use rayon::prelude::*;
use std::ops::{AddAssign, Index, IndexMut};

/// Dense row-major 2D array. `j` is the row (y), `i` the column (x).
#[derive(Clone, Debug)]
pub struct Grid2<T> {
    nx: usize,
    ny: usize,
    data: Vec<T>,
}

impl<T: Copy + Default> Grid2<T> {
    pub fn new(nx: usize, ny: usize) -> Self {
        Self { nx, ny, data: vec![T::default(); nx * ny] }
    }

    pub fn fill(&mut self, v: T) {
        self.data.fill(v);
    }

    /// Overwrite with another grid of the same size, reusing this allocation.
    pub fn copy_from(&mut self, other: &Self) {
        self.data.copy_from_slice(&other.data);
    }
}

impl<T> Grid2<T> {
    #[inline]
    pub fn idx(&self, i: usize, j: usize) -> usize {
        debug_assert!(i < self.nx && j < self.ny, "({i},{j}) out of {}x{}", self.nx, self.ny);
        j * self.nx + i
    }

    pub fn nx(&self) -> usize {
        self.nx
    }

    pub fn ny(&self) -> usize {
        self.ny
    }

    pub fn as_slice(&self) -> &[T] {
        &self.data
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.data
    }

    /// Rows as disjoint mutable slices, the basis of safe row-parallel loops.
    pub fn rows_mut(&mut self) -> std::slice::ChunksExactMut<'_, T> {
        self.data.chunks_exact_mut(self.nx)
    }
}

impl<T: Send> Grid2<T> {
    /// Rows as disjoint mutable slices, in parallel. Each closure owns its row
    /// exclusively, which is what lets the compiler accept the loop.
    ///
    /// (Forcing several rows per task was measured to be slower: the cost in the
    /// pressure solver is synchronising the pool ~600 times a frame, not task count.)
    pub fn par_rows_mut(&mut self) -> rayon::slice::ChunksExactMut<'_, T> {
        self.data.par_chunks_exact_mut(self.nx)
    }
}

impl<T: Copy + AddAssign> AddAssign<&Grid2<T>> for Grid2<T> {
    fn add_assign(&mut self, other: &Grid2<T>) {
        debug_assert_eq!((self.nx, self.ny), (other.nx, other.ny));
        for (a, &b) in self.data.iter_mut().zip(&other.data) {
            *a += b;
        }
    }
}

impl<T> Index<(usize, usize)> for Grid2<T> {
    type Output = T;

    #[inline]
    fn index(&self, (i, j): (usize, usize)) -> &T {
        &self.data[self.idx(i, j)]
    }
}

impl<T> IndexMut<(usize, usize)> for Grid2<T> {
    #[inline]
    fn index_mut(&mut self, (i, j): (usize, usize)) -> &mut T {
        let k = self.idx(i, j);
        &mut self.data[k]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexing_is_row_major() {
        let mut g = Grid2::<u32>::new(3, 2);
        g[(2, 1)] = 7;
        assert_eq!(g.as_slice()[5], 7);
        assert_eq!(g.rows_mut().count(), 2);
    }
}

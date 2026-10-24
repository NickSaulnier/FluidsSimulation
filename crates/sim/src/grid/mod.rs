use std::ops::{Index, IndexMut};

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

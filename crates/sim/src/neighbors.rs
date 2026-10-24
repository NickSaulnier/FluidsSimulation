//! Uniform-grid neighbour search via counting sort.
//!
//! All particles in bin `(i, j)` are `sorted[start[k]..start[k + 1]]` with
//! `k = j * nx + i`. Rebuilding is O(n) with no per-bin allocation, and the sorted order
//! keeps neighbours close in memory.

use glam::Vec2;

#[derive(Default)]
pub struct CellBins {
    cell: f32,
    nx: usize,
    ny: usize,
    start: Vec<u32>,
    sorted: Vec<u32>,
    /// Bin index of each particle from the last build (scratch).
    bin_of: Vec<u32>,
}

impl CellBins {
    /// Bin `pos` into square cells of size `cell` covering `[0, size]`.
    pub fn build(&mut self, pos: &[Vec2], cell: f32, size: Vec2) {
        self.cell = cell;
        self.nx = ((size.x / cell).ceil() as usize).max(1);
        self.ny = ((size.y / cell).ceil() as usize).max(1);
        let bins = self.nx * self.ny;

        // Count, then exclusive prefix sum, then scatter.
        self.start.clear();
        self.start.resize(bins + 1, 0);
        self.bin_of.clear();
        for &p in pos {
            let (i, j) = self.bin(p);
            let k = j * self.nx + i;
            self.bin_of.push(k as u32);
            self.start[k + 1] += 1;
        }
        for k in 0..bins {
            self.start[k + 1] += self.start[k];
        }
        self.sorted.clear();
        self.sorted.resize(pos.len(), 0);
        let mut next = self.start.clone();
        for (idx, &k) in self.bin_of.iter().enumerate() {
            let slot = &mut next[k as usize];
            self.sorted[*slot as usize] = idx as u32;
            *slot += 1;
        }
    }

    /// Bin coordinates of `p`, clamped onto the grid.
    #[inline]
    pub fn bin(&self, p: Vec2) -> (usize, usize) {
        let g = (p / self.cell).max(Vec2::ZERO);
        ((g.x as usize).min(self.nx - 1), (g.y as usize).min(self.ny - 1))
    }

    #[inline]
    pub fn in_bin(&self, i: usize, j: usize) -> &[u32] {
        let k = j * self.nx + i;
        &self.sorted[self.start[k] as usize..self.start[k + 1] as usize]
    }

    /// Calls `f` with every particle index in the 3x3 bins around `p`'s bin. With a bin
    /// size of at least the search radius, that covers every neighbour.
    #[inline]
    pub fn for_each_candidate(&self, p: Vec2, mut f: impl FnMut(usize)) {
        let (bi, bj) = self.bin(p);
        for j in bj.saturating_sub(1)..=(bj + 1).min(self.ny - 1) {
            for i in bi.saturating_sub(1)..=(bi + 1).min(self.nx - 1) {
                for &idx in self.in_bin(i, j) {
                    f(idx as usize);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::SplitMix64;

    #[test]
    fn candidates_cover_all_neighbours_like_brute_force() {
        let mut rng = SplitMix64::new(3);
        let size = Vec2::new(2.0, 1.0);
        let pos: Vec<Vec2> = (0..2000).map(|_| Vec2::new(rng.next_f32() * size.x, rng.next_f32() * size.y)).collect();
        let radius = 0.05;
        let mut bins = CellBins::default();
        bins.build(&pos, radius, size);

        for (a, &pa) in pos.iter().enumerate().step_by(37) {
            let mut fast = Vec::new();
            bins.for_each_candidate(pa, |b| {
                if b != a && pa.distance(pos[b]) < radius {
                    fast.push(b);
                }
            });
            let mut brute: Vec<usize> =
                (0..pos.len()).filter(|&b| b != a && pa.distance(pos[b]) < radius).collect();
            fast.sort_unstable();
            brute.sort_unstable();
            assert_eq!(fast, brute, "neighbours of particle {a}");
        }
    }

    #[test]
    fn every_particle_is_binned_exactly_once() {
        let pos = vec![Vec2::new(0.1, 0.1), Vec2::new(0.1, 0.1), Vec2::new(1.9, 0.9), Vec2::new(-1.0, 5.0)];
        let mut bins = CellBins::default();
        bins.build(&pos, 0.25, Vec2::new(2.0, 1.0));
        let mut seen: Vec<u32> = (0..bins.ny).flat_map(|j| (0..bins.nx).map(move |i| (i, j))).flat_map(|(i, j)| bins.in_bin(i, j).to_vec()).collect();
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3]);
    }
}

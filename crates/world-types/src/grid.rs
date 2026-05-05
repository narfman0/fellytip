//! Generic fixed-size 2D grid used by the nav grid, zone nav grids, and
//! anything else that wants flat row-major storage with bounds-checked
//! accessors.

/// Row-major 2D grid: `cells[y * w + x]`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Grid<T> {
    pub w: usize,
    pub h: usize,
    pub cells: Vec<T>,
}

impl<T: Clone + Default> Grid<T> {
    /// Construct a `w × h` grid filled with `T::default()`.
    pub fn new(w: usize, h: usize) -> Self {
        Grid {
            w,
            h,
            cells: vec![T::default(); w * h],
        }
    }
}

impl<T> Grid<T> {
    /// Construct a grid from a pre-built row-major cell vec.
    ///
    /// Panics if `cells.len() != w * h`.
    pub fn from_cells(w: usize, h: usize, cells: Vec<T>) -> Self {
        assert_eq!(cells.len(), w * h, "grid cell count mismatch");
        Grid { w, h, cells }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> &T {
        &self.cells[y * self.w + x]
    }

    #[inline]
    pub fn get_mut(&mut self, x: usize, y: usize) -> &mut T {
        &mut self.cells[y * self.w + x]
    }

    #[inline]
    pub fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h
    }

    /// 4-neighborhood: N, S, E, W (skipping out-of-bounds).
    pub fn neighbors_4(&self, x: usize, y: usize) -> impl Iterator<Item = (usize, usize)> + '_ {
        let w = self.w;
        let h = self.h;
        let mut buf = [(0usize, 0usize); 4];
        let mut n = 0usize;
        if x + 1 < w {
            buf[n] = (x + 1, y);
            n += 1;
        }
        if x > 0 {
            buf[n] = (x - 1, y);
            n += 1;
        }
        if y + 1 < h {
            buf[n] = (x, y + 1);
            n += 1;
        }
        if y > 0 {
            buf[n] = (x, y - 1);
            n += 1;
        }
        buf.into_iter().take(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_defaults() {
        let g: Grid<u32> = Grid::new(3, 4);
        assert_eq!(g.w, 3);
        assert_eq!(g.h, 4);
        assert_eq!(g.cells.len(), 12);
        assert_eq!(*g.get(2, 3), 0);
    }

    #[test]
    fn get_set() {
        let mut g: Grid<u32> = Grid::new(4, 4);
        *g.get_mut(1, 2) = 99;
        assert_eq!(*g.get(1, 2), 99);
    }

    #[test]
    fn in_bounds_checks() {
        let g: Grid<u32> = Grid::new(4, 4);
        assert!(g.in_bounds(0, 0));
        assert!(g.in_bounds(3, 3));
        assert!(!g.in_bounds(-1, 0));
        assert!(!g.in_bounds(4, 0));
        assert!(!g.in_bounds(0, 4));
    }

    #[test]
    fn neighbors_4_interior() {
        let g: Grid<u32> = Grid::new(4, 4);
        let ns: Vec<_> = g.neighbors_4(1, 1).collect();
        assert_eq!(ns.len(), 4);
    }

    #[test]
    fn neighbors_4_corner() {
        let g: Grid<u32> = Grid::new(4, 4);
        let ns: Vec<_> = g.neighbors_4(0, 0).collect();
        assert_eq!(ns.len(), 2);
    }
}

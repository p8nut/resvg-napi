//! What `toy` re-exports by name (`pub use toy_dep::Shade`): an enum another
//! crate defines, which a binding of `toy` reads as `toy::Shade`.
//!
//! The toy binding also reads it for its classes alone (`Role::Classes`):
//! `Grid` is listed, and the value types its members reach are classes too.

/// A colour by kind or by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Shade {
    /// No colour.
    Plain,
    /// A named colour.
    Named(String),
}

/// The bytes a cell takes. A constant of a crate read for its classes
/// alone: seen, reported, never emitted.
pub const CELL_BYTES: usize = 1;

/// A grid of one-byte cells, row by row: a class of a crate read for its
/// classes alone. What its members take and return, no config lists:
/// `Area` and `Cell` are classes because `Grid`'s members reach them, and
/// `Mark` because `Cell`'s do.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    width: u32,
    height: u32,
    cells: Vec<Cell>,
}

impl Grid {
    /// A `width` x `height` grid of empty cells; none when either is zero.
    pub fn new(width: u32, height: u32) -> Option<Grid> {
        (width > 0 && height > 0).then(|| Grid {
            width,
            height,
            cells: vec![Cell(0); (width * height) as usize],
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// The whole grid, as an area.
    pub fn bounds(&self) -> Area {
        Area {
            x: 0,
            y: 0,
            width: self.width,
            height: self.height,
        }
    }

    /// The cell at `(x, y)`; none outside the grid.
    pub fn cell(&self, x: u32, y: u32) -> Option<Cell> {
        (x < self.width && y < self.height).then(|| self.cells[(y * self.width + x) as usize])
    }

    /// Writes the cell at `(x, y)`; false outside the grid.
    pub fn set(&mut self, x: u32, y: u32, cell: Cell) -> bool {
        let inside = x < self.width && y < self.height;
        if inside {
            self.cells[(y * self.width + x) as usize] = cell;
        }
        inside
    }

    /// The cells of `area` as a grid of their own, clipped to this one; none
    /// when it misses the grid.
    pub fn crop(&self, area: Area) -> Option<Grid> {
        let (right, bottom) = (
            (area.x + area.width).min(self.width),
            (area.y + area.height).min(self.height),
        );
        let mut out = Grid::new(right.checked_sub(area.x)?, bottom.checked_sub(area.y)?)?;
        for y in area.y..bottom {
            for x in area.x..right {
                out.set(x - area.x, y - area.y, self.cell(x, y)?);
            }
        }
        Some(out)
    }

    /// Every cell, row by row. A slice of a value type: left out of JS,
    /// where an array of instances would be a poor copy.
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Takes a `Token`, which has nothing to call: left out of JS.
    pub fn stamp(&mut self, _token: Token) {}
}

/// A rectangle of cells, its fields private: a class with a static and
/// getters, reached by `Grid::bounds` and `Grid::crop`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl Area {
    /// None when it is empty.
    pub fn from_xywh(x: u32, y: u32, width: u32, height: u32) -> Option<Area> {
        (width > 0 && height > 0).then_some(Area {
            x,
            y,
            width,
            height,
        })
    }

    pub fn x(&self) -> u32 {
        self.x
    }

    pub fn y(&self) -> u32 {
        self.y
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }
}

/// One cell's byte, reached by `Grid::cell` and `Grid::set`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell(u8);

impl Cell {
    pub fn new(value: u8) -> Cell {
        Cell(value)
    }

    pub fn value(&self) -> u8 {
        self.0
    }

    /// Whether the cell is set: reached through `Cell` alone.
    pub fn mark(&self) -> Mark {
        Mark { on: self.0 != 0 }
    }
}

/// A cell's on/off state: plain data, reached only through `Cell::mark`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub on: bool,
}

impl Mark {
    pub fn flipped(&self) -> Mark {
        Mark { on: !self.on }
    }
}

/// `Copy`, but with nothing to call: never a class.
#[derive(Clone, Copy, Debug, Default)]
pub struct Token;

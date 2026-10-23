//! Rectangles in points with a top-left origin, as used by flipped views.

/// A rectangle in points; `y` grows downwards.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn min_x(&self) -> f64 {
        self.x
    }

    pub fn max_x(&self) -> f64 {
        self.x + self.w
    }

    pub fn mid_x(&self) -> f64 {
        self.x + self.w / 2.0
    }

    pub fn min_y(&self) -> f64 {
        self.y
    }

    pub fn max_y(&self) -> f64 {
        self.y + self.h
    }

    pub fn mid_y(&self) -> f64 {
        self.y + self.h / 2.0
    }

    /// The rectangle grown by `dy` above and below.
    pub fn outset_y(&self, dy: f64) -> Rect {
        Rect::new(self.x, self.y - dy, self.w, self.h + 2.0 * dy)
    }

    /// Whether `point` lies inside or on the boundary of this rectangle.
    pub fn contains(&self, point: Point) -> bool {
        self.w > 0.0
            && self.h > 0.0
            && point.x >= self.x
            && point.x <= self.max_x()
            && point.y >= self.y
            && point.y <= self.max_y()
    }
}

/// A 2D point in points; `y` grows downwards.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// A width and height in points.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

impl Size {
    pub const fn new(w: f64, h: f64) -> Self {
        Self { w, h }
    }
}

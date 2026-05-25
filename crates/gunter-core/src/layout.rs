use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Axis { Horizontal, Vertical }

#[derive(Clone, Copy, Debug, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self { Rect { x, y, w, h } }

    pub fn cols(&self, cell_w: f32) -> u16 { (self.w / cell_w).max(1.0) as u16 }
    pub fn rows(&self, cell_h: f32) -> u16 { (self.h / cell_h).max(1.0) as u16 }

    pub fn split(self, axis: Axis, ratio: f32) -> (Rect, Rect) {
        match axis {
            Axis::Horizontal => {
                let left_w = self.w * ratio;
                (
                    Rect::new(self.x, self.y, left_w, self.h),
                    Rect::new(self.x + left_w, self.y, self.w - left_w, self.h),
                )
            }
            Axis::Vertical => {
                let top_h = self.h * ratio;
                (
                    Rect::new(self.x, self.y, self.w, top_h),
                    Rect::new(self.x, self.y + top_h, self.w, self.h - top_h),
                )
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum Layout {
    Leaf(Uuid),
    Split {
        axis: Axis,
        ratio: f32,
        left: Box<Layout>,
        right: Box<Layout>,
    },
}

impl Layout {
    pub fn leaf(id: Uuid) -> Self { Layout::Leaf(id) }

    /// Walk and collect (session_id, rect) pairs.
    pub fn rects(&self, rect: Rect) -> Vec<(Uuid, Rect)> {
        match self {
            Layout::Leaf(id) => vec![(*id, rect)],
            Layout::Split { axis, ratio, left, right } => {
                let (lr, rr) = rect.split(*axis, *ratio);
                let mut out = left.rects(lr);
                out.extend(right.rects(rr));
                out
            }
        }
    }

    /// Find the focused leaf's ID (leftmost for now).
    pub fn focused(&self) -> Uuid {
        match self {
            Layout::Leaf(id) => *id,
            Layout::Split { left, .. } => left.focused(),
        }
    }

    /// Insert a split at the leaf with `target_id`.
    pub fn split_leaf(self, target_id: Uuid, axis: Axis, new_id: Uuid) -> Self {
        match self {
            Layout::Leaf(id) if id == target_id => Layout::Split {
                axis,
                ratio: 0.5,
                left: Box::new(Layout::Leaf(id)),
                right: Box::new(Layout::Leaf(new_id)),
            },
            Layout::Split { axis: a, ratio, left, right } => Layout::Split {
                axis: a,
                ratio,
                left: Box::new(left.split_leaf(target_id, axis, new_id)),
                right: Box::new(right.split_leaf(target_id, axis, new_id)),
            },
            other => other,
        }
    }

    /// Remove a leaf and replace split with remaining sibling.
    pub fn remove_leaf(self, target_id: Uuid) -> Option<Self> {
        match self {
            Layout::Leaf(id) if id == target_id => None,
            Layout::Leaf(_) => Some(self),
            Layout::Split { axis, ratio, left, right } => {
                match (left.remove_leaf(target_id), right.remove_leaf(target_id)) {
                    (None, Some(r)) => Some(r),
                    (Some(l), None) => Some(l),
                    (Some(l), Some(r)) => Some(Layout::Split {
                        axis, ratio,
                        left: Box::new(l),
                        right: Box::new(r),
                    }),
                    (None, None) => None,
                }
            }
        }
    }
}

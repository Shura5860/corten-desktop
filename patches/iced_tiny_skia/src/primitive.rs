use crate::core::Rectangle;

#[derive(Debug, Clone, PartialEq)]
pub enum Primitive {
    /// A path filled with some paint.
    Fill {
        /// The path to fill.
        path: tiny_skia::Path,
        /// The paint to use.
        paint: tiny_skia::Paint<'static>,
        /// The fill rule to follow.
        rule: tiny_skia::FillRule,
    },
    /// A path stroked with some paint.
    Stroke {
        /// The path to stroke.
        path: tiny_skia::Path,
        /// The paint to use.
        paint: tiny_skia::Paint<'static>,
        /// The stroke settings.
        stroke: tiny_skia::Stroke,
    },
}

impl Primitive {
    /// Returns the visible bounds of the [`Primitive`].
    pub fn visible_bounds(&self) -> Rectangle {
        let (bounds, pad) = match self {
            Primitive::Fill { path, .. } => (path.bounds(), 2.0),
            Primitive::Stroke { path, stroke, .. } => {
                (path.bounds(), (stroke.width * 0.5) + 2.0)
            }
        };

        Rectangle {
            x: bounds.x() - pad,
            y: bounds.y() - pad,
            width: bounds.width() + pad * 2.0,
            height: bounds.height() + pad * 2.0,
        }
    }
}

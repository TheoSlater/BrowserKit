use crate::{Error, ErrorKind, Result};
use serde::{Deserialize, Serialize};

/// Coordinate system for a rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinateSpace {
    /// CSS pixels reported by future frontend geometry integration.
    FrontendLogical,
    /// BrowserKit logical pixels. Current page bounds use this space.
    WindowLogical,
    /// Device pixels used only by native platform backends.
    Physical,
}

/// Floating-point rectangle in logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Rectangle plus its declared coordinate space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoordinateRect {
    pub rect: LogicalRect,
    pub space: CoordinateSpace,
}

impl LogicalRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Result<Self> {
        let rect = Self {
            x,
            y,
            width,
            height,
        };
        rect.validate()?;
        Ok(rect)
    }

    pub fn validate(self) -> Result<Self> {
        if [self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f64::is_finite)
            && self.width >= 0.0
            && self.height >= 0.0
        {
            Ok(self)
        } else {
            Err(Error::new(
                ErrorKind::Geometry,
                "bounds must be finite with non-negative size",
            ))
        }
    }

    pub fn space(self) -> CoordinateSpace {
        CoordinateSpace::WindowLogical
    }

    pub fn in_space(self, space: CoordinateSpace) -> CoordinateRect {
        CoordinateRect { rect: self, space }
    }
}

/// Scale metadata used when converting BrowserKit logical geometry to native pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleContext {
    pub window_scale_factor: f64,
}

impl ScaleContext {
    pub fn new(window_scale_factor: f64) -> Result<Self> {
        if window_scale_factor.is_finite() && window_scale_factor > 0.0 {
            Ok(Self {
                window_scale_factor,
            })
        } else {
            Err(Error::new(
                ErrorKind::Geometry,
                "window scale factor must be finite and positive",
            ))
        }
    }

    /// Convert logical geometry at the backend boundary. Rounding belongs here, not shared code.
    pub fn to_physical(self, rect: LogicalRect) -> Result<PhysicalRect> {
        rect.validate()?;
        Ok(PhysicalRect {
            x: (rect.x * self.window_scale_factor).round() as i32,
            y: (rect.y * self.window_scale_factor).round() as i32,
            width: (rect.width * self.window_scale_factor).round() as u32,
            height: (rect.height * self.window_scale_factor).round() as u32,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_logical_geometry() {
        assert!(LogicalRect::new(0.0, 0.0, 1.5, 2.5).is_ok());
        assert!(LogicalRect::new(0.0, 0.0, -1.0, 1.0).is_err());
        assert!(LogicalRect::new(f64::NAN, 0.0, 1.0, 1.0).is_err());
    }

    #[test]
    fn converts_fractional_geometry_at_backend_boundary() {
        let scale = ScaleContext::new(1.5).unwrap();
        assert_eq!(
            scale
                .to_physical(LogicalRect::new(1.0, 2.0, 3.0, 4.0).unwrap())
                .unwrap(),
            PhysicalRect {
                x: 2,
                y: 3,
                width: 5,
                height: 6
            }
        );
    }

    #[test]
    fn rejects_invalid_scale() {
        assert!(ScaleContext::new(0.0).is_err());
        assert!(ScaleContext::new(f64::INFINITY).is_err());
    }
}

/// Integer native rectangle. Conversion is backend-owned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysicalRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

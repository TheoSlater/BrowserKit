//! CEF-independent types shared by BrowserKit's public layers.

pub mod protocol;

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Copy, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            pub fn from_raw(value: u64) -> Self {
                Self(value)
            }
            pub fn get(self) -> u64 {
                self.0
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .debug_tuple(stringify!($name))
                    .field(&self.0)
                    .finish()
            }
        }
    };
}

id_type!(WindowId);
id_type!(PageId);

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl LogicalRect {
    pub fn validate(self) -> Result<(), &'static str> {
        if !self.x.is_finite()
            || !self.y.is_finite()
            || !self.width.is_finite()
            || !self.height.is_finite()
        {
            return Err("logical rect values must be finite");
        }
        if self.width < 0.0 || self.height < 0.0 {
            return Err("logical rect dimensions must not be negative");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinateSpace {
    FrontendLogical,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageViewBounds {
    pub rect: LogicalRect,
    pub coordinate_space: CoordinateSpace,
    pub device_pixel_ratio: f64,
    pub visual_viewport_scale: f64,
}

impl PageViewBounds {
    pub fn validate(self) -> Result<(), &'static str> {
        self.rect.validate()?;
        if !self.device_pixel_ratio.is_finite() || self.device_pixel_ratio <= 0.0 {
            return Err("device pixel ratio must be finite and positive");
        }
        if !self.visual_viewport_scale.is_finite() || self.visual_viewport_scale <= 0.0 {
            return Err("visual viewport scale must be finite and positive");
        }
        Ok(())
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    fn bounds(rect: LogicalRect) -> PageViewBounds {
        PageViewBounds {
            rect,
            coordinate_space: CoordinateSpace::FrontendLogical,
            device_pixel_ratio: 1.0,
            visual_viewport_scale: 1.0,
        }
    }

    #[test]
    fn accepts_fractional_and_zero_size_rects() {
        assert!(bounds(LogicalRect {
            x: 1.25,
            y: 2.5,
            width: 0.0,
            height: 4.75
        })
        .validate()
        .is_ok());
    }

    #[test]
    fn rejects_non_finite_and_negative_geometry() {
        assert!(bounds(LogicalRect {
            x: f64::NAN,
            y: 0.0,
            width: 1.0,
            height: 1.0
        })
        .validate()
        .is_err());
        assert!(bounds(LogicalRect {
            x: 0.0,
            y: 0.0,
            width: f64::INFINITY,
            height: 1.0
        })
        .validate()
        .is_err());
        assert!(bounds(LogicalRect {
            x: 0.0,
            y: 0.0,
            width: -1.0,
            height: 1.0
        })
        .validate()
        .is_err());
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageOptions {
    pub url: String,
}

impl PageOptions {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PageState {
    pub id: PageId,
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub active: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub id: WindowId,
    pub active_page_id: Option<PageId>,
    pub pages: Vec<PageState>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BrowserEvent {
    PageUrlChanged {
        page_id: PageId,
        url: String,
    },
    PageTitleChanged {
        page_id: PageId,
        title: String,
    },
    PageLoadingChanged {
        page_id: PageId,
        loading: bool,
    },
    PageNavigationStateChanged {
        page_id: PageId,
        can_go_back: bool,
        can_go_forward: bool,
    },
    PageActivated {
        window_id: WindowId,
        page_id: PageId,
    },
    PageClosed {
        window_id: WindowId,
        page_id: PageId,
    },
}

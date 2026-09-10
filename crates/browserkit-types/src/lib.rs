//! Shared platform-independent BrowserKit types.

mod error;
mod geometry;
mod ids;
mod options;
pub mod protocol;

pub use error::{Error, ErrorKind, Result};
pub use geometry::{CoordinateRect, CoordinateSpace, LogicalRect, PhysicalRect, ScaleContext};
pub use ids::{PageId, WindowId};
pub use options::{BrowserOptions, PageOptions, WebViewHostMode, WindowOptions};
pub use protocol::{
    Command, Event, FrontendMessage, NativeMessage, PageState, ProtocolError, Request,
    ResponseData, WindowState, PROTOCOL_VERSION,
};

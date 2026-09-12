use super::{
    BrowserEvent, CoordinateSpace, LogicalRect, PageId, PageOptions, PageState, WindowId,
    WindowState,
};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub String);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FrontendMessage {
    Command(CommandEnvelope),
    Request(RequestEnvelope),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandEnvelope {
    pub command: Command,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub id: RequestId,
    pub request: Request,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    PageNavigate {
        page_id: PageId,
        url: String,
    },
    PageReload {
        page_id: PageId,
    },
    PageGoBack {
        page_id: PageId,
    },
    PageGoForward {
        page_id: PageId,
    },
    PageStop {
        page_id: PageId,
    },
    PageActivate {
        window_id: WindowId,
        page_id: PageId,
    },
    PageClose {
        window_id: WindowId,
        page_id: PageId,
    },
    PageRegisterView {
        page_id: PageId,
    },
    PageSetViewBounds {
        page_id: PageId,
        rect: LogicalRect,
        coordinate_space: CoordinateSpace,
        device_pixel_ratio: f64,
        visual_viewport_scale: f64,
    },
    PageUnregisterView {
        page_id: PageId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    RuntimeHandshake {
        protocol_version: u32,
    },
    PageCreate {
        window_id: WindowId,
        options: PageOptions,
    },
    PageGetState {
        page_id: PageId,
    },
    WindowGetState {
        window_id: WindowId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeMessage {
    Response(ResponseEnvelope),
    Event(EventEnvelope),
}

pub trait ProtocolSink: Send + Sync {
    fn send(&self, message: NativeMessage);
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub id: RequestId,
    pub result: ResponseResult,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ResponseResult {
    Ok { data: ResponseData },
    Err { error: ProtocolError },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseData {
    RuntimeHandshake {
        protocol_version: u32,
        windows: Vec<WindowState>,
    },
    PageCreated {
        page: PageState,
    },
    PageState {
        page: PageState,
    },
    WindowState {
        window: WindowState,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub event: Event,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    PageCreated {
        page: PageState,
    },
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

impl From<BrowserEvent> for Event {
    fn from(event: BrowserEvent) -> Self {
        match event {
            BrowserEvent::PageUrlChanged { page_id, url } => Self::PageUrlChanged { page_id, url },
            BrowserEvent::PageTitleChanged { page_id, title } => {
                Self::PageTitleChanged { page_id, title }
            }
            BrowserEvent::PageLoadingChanged { page_id, loading } => {
                Self::PageLoadingChanged { page_id, loading }
            }
            BrowserEvent::PageNavigationStateChanged {
                page_id,
                can_go_back,
                can_go_forward,
            } => Self::PageNavigationStateChanged {
                page_id,
                can_go_back,
                can_go_forward,
            },
            BrowserEvent::PageActivated { window_id, page_id } => {
                Self::PageActivated { window_id, page_id }
            }
            BrowserEvent::PageClosed { window_id, page_id } => {
                Self::PageClosed { window_id, page_id }
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: ProtocolErrorCode,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolErrorCode {
    InvalidMessage,
    UnsupportedMessage,
    ProtocolVersionMismatch,
    WindowNotFound,
    PageNotFound,
    InvalidUrl,
    RuntimeNotReady,
    RuntimeShuttingDown,
    RequestFailed,
    InvalidViewBounds,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_use_stable_wire_values() {
        let message = FrontendMessage::Request(RequestEnvelope {
            id: RequestId("req-42".into()),
            request: Request::PageGetState {
                page_id: PageId::from_raw(7),
            },
        });
        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("req-42"));
        assert!(json.contains("\"page_id\":7"));
        assert_eq!(
            serde_json::from_str::<FrontendMessage>(&json).unwrap(),
            message
        );
    }

    #[test]
    fn every_envelope_round_trips() {
        let message = NativeMessage::Event(EventEnvelope {
            event: Event::PageLoadingChanged {
                page_id: PageId::from_raw(1),
                loading: true,
            },
        });
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(
            serde_json::from_str::<NativeMessage>(&json).unwrap(),
            message
        );
    }

    #[test]
    fn protocol_version_is_single_source_of_truth() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }

    #[test]
    fn view_bounds_keep_wire_names_and_fractional_values() {
        let message = FrontendMessage::Command(CommandEnvelope {
            command: Command::PageSetViewBounds {
                page_id: PageId::from_raw(1),
                rect: LogicalRect {
                    x: 12.5,
                    y: 24.0,
                    width: 640.25,
                    height: 480.0,
                },
                coordinate_space: CoordinateSpace::FrontendLogical,
                device_pixel_ratio: 2.0,
                visual_viewport_scale: 1.0,
            },
        });
        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains("page_set_view_bounds"));
        assert!(json.contains("frontend_logical"));
        assert_eq!(
            serde_json::from_str::<FrontendMessage>(&json).unwrap(),
            message
        );
    }
}

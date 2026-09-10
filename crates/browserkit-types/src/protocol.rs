use serde::{Deserialize, Serialize};

use crate::{CoordinateSpace, LogicalRect, PageId};

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FrontendMessage {
    Command { command: Command },
    Request { id: String, request: Request },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NativeMessage {
    Event {
        event: Event,
    },
    Response {
        id: String,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<ResponseData>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ProtocolError>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    #[serde(rename = "page.navigate")]
    PageNavigate {
        #[serde(rename = "pageId")]
        page_id: Option<PageId>,
        url: String,
    },
    #[serde(rename = "page.reload")]
    PageReload {
        #[serde(rename = "pageId")]
        page_id: Option<PageId>,
    },
    #[serde(rename = "page.go_back")]
    PageGoBack {
        #[serde(rename = "pageId")]
        page_id: Option<PageId>,
    },
    #[serde(rename = "page.go_forward")]
    PageGoForward {
        #[serde(rename = "pageId")]
        page_id: Option<PageId>,
    },
    #[serde(rename = "page.activate")]
    PageActivate {
        #[serde(rename = "pageId")]
        page_id: PageId,
    },
    #[serde(rename = "page.close")]
    PageClose {
        #[serde(rename = "pageId")]
        page_id: PageId,
    },
    #[serde(rename = "page.set_view_bounds")]
    PageSetViewBounds {
        #[serde(rename = "pageId")]
        page_id: PageId,
        rect: LogicalRect,
        #[serde(rename = "coordinateSpace")]
        coordinate_space: CoordinateSpace,
        #[serde(rename = "devicePixelRatio")]
        device_pixel_ratio: f64,
        #[serde(rename = "visualViewportScale")]
        visual_viewport_scale: f64,
    },
    #[serde(rename = "page.set_visible")]
    PageSetVisible {
        #[serde(rename = "pageId")]
        page_id: PageId,
        visible: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    #[serde(rename = "page.create")]
    PageCreate { url: Option<String> },
    #[serde(rename = "page.get_state")]
    PageGetState {
        #[serde(rename = "pageId")]
        page_id: Option<PageId>,
    },
    #[serde(rename = "window.get_state")]
    WindowGetState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    #[serde(rename = "page.created")]
    PageCreated { state: PageState },
    #[serde(rename = "page.closed")]
    PageClosed {
        #[serde(rename = "pageId")]
        page_id: PageId,
    },
    #[serde(rename = "page.activated")]
    PageActivated { state: PageState },
    #[serde(rename = "page.url_changed")]
    PageUrlChanged {
        #[serde(rename = "pageId")]
        page_id: PageId,
        url: Option<String>,
    },
    #[serde(rename = "page.title_changed")]
    PageTitleChanged {
        #[serde(rename = "pageId")]
        page_id: PageId,
        title: Option<String>,
    },
    #[serde(rename = "page.loading_changed")]
    PageLoadingChanged {
        #[serde(rename = "pageId")]
        page_id: PageId,
        loading: bool,
    },
    #[serde(rename = "page.navigation_state_changed")]
    PageNavigationStateChanged {
        #[serde(rename = "pageId")]
        page_id: PageId,
        #[serde(rename = "canGoBack")]
        can_go_back: bool,
        #[serde(rename = "canGoForward")]
        can_go_forward: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseData {
    #[serde(rename = "page.created")]
    PageCreated {
        #[serde(rename = "pageId")]
        page_id: PageId,
    },
    #[serde(rename = "page.state")]
    PageState { state: PageState },
    #[serde(rename = "window.state")]
    WindowState { state: WindowState },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageState {
    #[serde(rename = "pageId")]
    pub id: PageId,
    pub url: Option<String>,
    pub title: Option<String>,
    pub loading: bool,
    #[serde(rename = "canGoBack")]
    pub can_go_back: bool,
    #[serde(rename = "canGoForward")]
    pub can_go_forward: bool,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    #[serde(rename = "activePageId")]
    pub active_page_id: Option<PageId>,
    pub pages: Vec<PageState>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_round_trips_stable_ids() {
        let message = FrontendMessage::Command {
            command: Command::PageActivate {
                page_id: PageId::new(42),
            },
        };
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(
            json,
            r#"{"type":"command","command":{"type":"page.activate","pageId":42}}"#
        );
        assert_eq!(
            serde_json::from_str::<FrontendMessage>(&json).unwrap(),
            message
        );
    }

    #[test]
    fn protocol_version_is_explicit() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }

    #[test]
    fn request_response_and_event_round_trip() {
        let messages = [
            NativeMessage::Response {
                id: "req-42".into(),
                ok: true,
                data: Some(ResponseData::PageCreated {
                    page_id: PageId::new(3),
                }),
                error: None,
            },
            NativeMessage::Event {
                event: Event::PageNavigationStateChanged {
                    page_id: PageId::new(3),
                    can_go_back: true,
                    can_go_forward: false,
                },
            },
        ];

        for message in messages {
            let json = serde_json::to_string(&message).unwrap();
            assert_eq!(
                serde_json::from_str::<NativeMessage>(&json).unwrap(),
                message
            );
        }
    }

    #[test]
    fn request_round_trips() {
        let message = FrontendMessage::Request {
            id: "req-1".into(),
            request: Request::PageGetState {
                page_id: Some(PageId::new(7)),
            },
        };
        let json = serde_json::to_string(&message).unwrap();
        assert_eq!(
            serde_json::from_str::<FrontendMessage>(&json).unwrap(),
            message
        );
    }
}

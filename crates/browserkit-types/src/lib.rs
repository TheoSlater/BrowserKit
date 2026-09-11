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
